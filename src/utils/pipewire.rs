use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Cursor;
use std::iter::zip;
use std::os::fd::{AsFd, AsRawFd};
use std::rc::Rc;
use std::sync::atomic::{self, AtomicUsize};
use std::time::Duration;

use anyhow::Context;
use pipewire::properties::Properties;
use pipewire::spa::buffer::DataType;
use pipewire::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pipewire::spa::param::format_utils::parse_format;
use pipewire::spa::param::video::{VideoFormat, VideoInfoRaw};
use pipewire::spa::param::ParamType;
use pipewire::spa::pod::deserialize::PodDeserializer;
use pipewire::spa::pod::serialize::PodSerializer;
use pipewire::spa::pod::{self, ChoiceValue, Pod, PodPropFlags, Property, PropertyFlags};
use pipewire::spa::sys::{
    SPA_PARAM_BUFFERS_blocks, SPA_PARAM_BUFFERS_buffers, SPA_PARAM_BUFFERS_dataType,
    SPA_DATA_FLAG_READWRITE,
};
use pipewire::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Rectangle, SpaTypes};
use pipewire::stream::{Stream, StreamFlags, StreamState};
use smithay::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::{GbmBuffer, GbmDevice};
use smithay::backend::allocator::{Format, Fourcc};
use smithay::backend::drm::DrmDeviceFd;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::RenderElement;
use smithay::output::{OutputModeSource, WeakOutput};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{
    self, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
};
use smithay::reexports::gbm::{BufferObjectFlags as GbmBufferFlags, Modifier};
use smithay::utils::{Physical, Scale, Size, Transform};
use zvariant::OwnedObjectPath;

use crate::portals::screencast::{CursorMode, StreamMetadata};
use crate::renderer::{FhtRenderElement, FhtRenderer, OutputElementsResult};
use crate::state::State;
use crate::window::WeakWindow;

pub struct PipeWire {
    _context: pipewire::context::Context,
    core: pipewire::core::Core,
    pub casts: Vec<Cast>,
}

macro_rules! make_params {
    ($params:ident, $formats:expr, $size:expr, $refresh:expr, $alpha:expr) => {
        let mut b1 = Vec::new();
        let mut b2 = Vec::new();

        let o1 = make_video_object_params($size, $formats, $refresh, false);
        let pod1 = make_pod(&mut b1, o1);

        let mut p1;
        let mut p2;
        $params = if $alpha {
            let o2 = make_video_object_params($size, $formats, $refresh, true);
            p2 = [pod1, make_pod(&mut b2, o2)];
            &mut p2[..]
        } else {
            p1 = [pod1];
            &mut p1[..]
        };
    };
}

// We communicate between the pipewire instance, the compositor, and the portal using channels.
// They all live in different threads.
pub enum PwToCompositor {
    Redraw { id: CastId, source: CastSource },
    StopCast { id: CastId },
}

impl PipeWire {
    pub fn new(loop_handle: &LoopHandle<'static, State>) -> anyhow::Result<Self> {
        pipewire::init();

        // Initial the main loop.
        let main_loop = pipewire::main_loop::MainLoop::new(None)
            .context("Failed to initialize pipewire main loop!")?;
        let context = pipewire::context::Context::new(&main_loop)
            .context("Failed to initialize pipewire context")?;
        // Logging
        let core = context
            .connect(None)
            .context("Failed to connect pipewire context!")?;
        let listener = core
            .add_listener_local()
            .error(|id, seq, res, message| warn!(?id, ?seq, ?res, ?message, "PipeWire error"))
            .register();
        std::mem::forget(listener);

        struct MainLoopWrapper(pipewire::main_loop::MainLoop);
        impl AsFd for MainLoopWrapper {
            fn as_fd(&self) -> std::os::unix::prelude::BorrowedFd<'_> {
                self.0.loop_().fd()
            }
        }
        let main_loop = MainLoopWrapper(main_loop);
        let source = Generic::new(main_loop, Interest::READ, Mode::Level);
        loop_handle
            .insert_source(source, |_, main_loop, _| {
                crate::profile_scope!("pipewire_loop_dispatch");
                main_loop.0.loop_().iterate(Duration::ZERO);
                Ok(PostAction::Continue)
            })
            .map_err(|err| anyhow::anyhow!("Failed to insert PipeWire event source! {err}"))?;

        Ok(Self {
            _context: context,
            core,
            casts: vec![],
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_cast(
        &mut self,
        // Information about the screencast request
        session_handle: OwnedObjectPath,
        source: CastSource,
        cursor_mode: CursorMode,
        // Communication
        to_compositor: calloop::channel::Sender<PwToCompositor>,
        to_compositor_token: RegistrationToken,
        metadata_sender: async_channel::Sender<Option<StreamMetadata>>,
        // Rendering information
        gbm: GbmDevice<DrmDeviceFd>,
        render_formats: &FormatSet,
        alpha: bool,
        size: smithay::utils::Size<i32, Physical>,
        refresh: u32,
    ) -> anyhow::Result<()> {
        crate::profile_function!();
        let size = smithay::utils::Size::from((size.w as u32, size.h as u32));
        let cast_id = CastId::unique();
        let inner = Rc::new(RefCell::new(CastInner {
            active: true,
            node_id: None,
            dmabufs: HashMap::new(),
            refresh,
            state: CastState::ResizePending { pending_size: size },
        }));

        let stream = Stream::new(
            &self.core,
            &format!("fht-compositor-{cast_id:?}"),
            Properties::new(),
        )
        .context("Error creating new PipeWire Stream!")?;

        let to_compositor_ = to_compositor.clone();
        let stop_cast = move || {
            let _ = to_compositor_.send(PwToCompositor::StopCast { id: cast_id });
        };

        let to_compositor_ = to_compositor.clone();
        let source_ = source.clone();
        let redraw = move || {
            let _ = to_compositor_.send(PwToCompositor::Redraw {
                id: cast_id,
                source: source_.clone(),
            });
        };

        let listener = stream
            .add_local_listener_with_user_data(Rc::clone(&inner))
            .state_changed({
                let stop_cast = stop_cast.clone();
                let redraw = redraw.clone();
                move |stream, inner, old, new| {
                    crate::profile_scope!(
                        "pw_stream::state_changed",
                        &stream.node_id().to_string()
                    );

                    let span = debug_span!("pw_stream");
                    span.record("node_id", stream.node_id());
                    let _guard = span.enter();

                    let mut inner = inner.borrow_mut();
                    debug!(?old, ?new, "state_changed");

                    match new {
                        StreamState::Unconnected => stop_cast(), // client gone
                        StreamState::Connecting => inner.active = false,
                        StreamState::Streaming => {
                            inner.active = true;
                            redraw();
                        }
                        StreamState::Paused => {
                            if inner.node_id.is_none() {
                                let node_id = stream.node_id();
                                inner.node_id = Some(node_id);

                                // Send new metadata to screencast portal to inform client
                                let metadata = StreamMetadata {
                                    cast_id,
                                    node_id,
                                    size,
                                };
                                if metadata_sender.try_send(Some(metadata)).is_err() {
                                    error!("failed to send stream metadata to portal, stopping");
                                    stop_cast();
                                }
                            }

                            inner.active = false;
                        }
                        StreamState::Error(err) => {
                            warn!("PipeWire stream error: {err}");
                            stop_cast();
                        }
                    }
                }
            })
            .param_changed({
                let render_formats = render_formats.clone();
                let stop_cast = stop_cast.clone();
                let gbm = gbm.clone();
                move |stream, inner, id, pod| {
                    crate::profile_scope!(
                        "pw_stream::param_changed",
                        &stream.node_id().to_string()
                    );

                    let span = debug_span!("pw_stream");
                    span.record("node_id", stream.node_id());
                    let _guard = span.enter();

                    debug!("param_changed");

                    let mut inner = inner.borrow_mut();
                    let id = ParamType::from_raw(id);

                    if id != ParamType::Format {
                        return;
                    }

                    let Some(pod) = pod else { return };

                    let (m_type, m_subtype) = match parse_format(pod) {
                        Ok(x) => x,
                        Err(err) => {
                            warn!(?err, "failed to parse format from pod");
                            return;
                        }
                    };

                    if m_type != MediaType::Video || m_subtype != MediaSubtype::Raw {
                        return;
                    }

                    let mut format = VideoInfoRaw::new();
                    format.parse(pod).unwrap();
                    debug!(?format, "requested format");

                    let format_size = Size::from((format.size().width, format.size().height));

                    if format_size != inner.state.expected_format_size() {
                        if !matches!(&inner.state, CastState::ResizePending { .. }) {
                            warn!("got wrong size, but we're not resizing");
                            stop_cast();
                            return;
                        }

                        warn!("size does not match, waiting...");
                        return;
                    }

                    let format_has_alpha = format.format() == VideoFormat::BGRA;
                    let fourcc = if format_has_alpha {
                        Fourcc::Argb8888
                    } else {
                        Fourcc::Xrgb8888
                    };

                    // Modifier negotiaction procedure:
                    //
                    // We only support dmabuf-based screencasting. The pipewire dmabuf docs say that
                    // if there's no VideoModifier param, we must fallback to SHM-based streaming,
                    // which we don't support since its HORRIBLE for performance.
                    let object = pod.as_object().unwrap();
                    let Some(prop_modifier) = object
                        .find_prop(pipewire::spa::utils::Id(FormatProperties::VideoModifier.0))
                    else {
                        warn!("client did not negociate modifiers, stopping");
                        stop_cast();
                        return;
                    };

                    if prop_modifier.flags().contains(PodPropFlags::DONT_FIXATE) {
                        // modifier-aware negociation.
                        //
                        // We get the list of modifiers that the client advertise and then run a
                        // test allocation using the GBM device. We use the first modifier that
                        // results in a successfull allocation
                        debug!("fixating modifier");

                        let pod_modifier = prop_modifier.value();
                        let Ok((_, modifiers)) = PodDeserializer::deserialize_from::<Choice<i64>>(
                            pod_modifier.as_bytes(),
                        ) else {
                            warn!("Client did not set correct modifier prop");
                            stop_cast();
                            return;
                        };

                        let ChoiceEnum::Enum { alternatives, .. } = modifiers.1 else {
                            warn!("client did not specify correct choice kind");
                            stop_cast();
                            return;
                        };

                        let (modifier, plane_count) = match find_preferred_modifier(
                            &gbm,
                            format_size,
                            fourcc,
                            alternatives,
                        ) {
                            Ok(x) => x,
                            Err(err) => {
                                warn!(?err, "couldn't find preferred modifier");
                                stop_cast();
                                return;
                            }
                        };

                        debug!(?modifier, ?plane_count, "allocation successful");

                        inner.state = CastState::ConfirmationPending {
                            size: format_size,
                            alpha: format_has_alpha,
                            modifier,
                            plane_count: plane_count as i32,
                        };

                        let fixated_format = FormatSet::from_iter([Format {
                            code: fourcc,
                            modifier,
                        }]);

                        let mut b1 = Vec::new();
                        let mut b2 = Vec::new();

                        let o1 = make_video_object_params(
                            format_size,
                            &fixated_format,
                            inner.refresh,
                            format_has_alpha,
                        );
                        let pod1 = make_pod(&mut b1, o1);

                        let o2 = make_video_object_params(
                            format_size,
                            &render_formats,
                            inner.refresh,
                            format_has_alpha,
                        );
                        let mut params = [pod1, make_pod(&mut b2, o2)];

                        if let Err(err) = stream.update_params(&mut params) {
                            warn!("error updating stream params: {err:?}");
                            stop_cast();
                        }

                        return;
                    }

                    let plane_count = match &inner.state {
                        CastState::ConfirmationPending {
                            size,
                            alpha,
                            modifier,
                            plane_count,
                        }
                        | CastState::Ready {
                            size,
                            alpha,
                            modifier,
                            plane_count,
                            ..
                        } if *alpha == format_has_alpha
                            && *modifier == Modifier::from(format.modifier()) =>
                        {
                            // The client didn't request new params, we can start rendering
                            let size = *size;
                            let alpha = *alpha;
                            let modifier = *modifier;
                            let plane_count = *plane_count;

                            let damage_tracker =
                                if let CastState::Ready { damage_tracker, .. } = &mut inner.state {
                                    // keep damage tracker around
                                    damage_tracker.take()
                                } else {
                                    None
                                };

                            trace!(id = ?cast_id, "cast is ready");

                            inner.state = CastState::Ready {
                                size,
                                alpha,
                                modifier,
                                plane_count,
                                damage_tracker,
                            };

                            plane_count
                        }
                        _ => {
                            // We're negotiating a single modifier, or alpha or modifier changed,
                            // so we need to do a test allocation.
                            let (modifier, plane_count) = match find_preferred_modifier(
                                &gbm,
                                format_size,
                                fourcc,
                                vec![format.modifier() as i64],
                            ) {
                                Ok(x) => x,
                                Err(err) => {
                                    warn!(?err, "test allocation failed");
                                    stop_cast();
                                    return;
                                }
                            };

                            debug!(?modifier, ?plane_count, "allocation successful");

                            inner.state = CastState::Ready {
                                size: format_size,
                                alpha: format_has_alpha,
                                modifier,
                                plane_count: plane_count as i32,
                                damage_tracker: None,
                            };

                            plane_count as i32
                        }
                    };

                    let o1 = pod::object!(
                        SpaTypes::ObjectParamBuffers,
                        ParamType::Buffers,
                        Property::new(
                            SPA_PARAM_BUFFERS_buffers,
                            pod::Value::Choice(ChoiceValue::Int(Choice(
                                ChoiceFlags::empty(),
                                ChoiceEnum::Range {
                                    default: 16,
                                    min: 2,
                                    max: 16
                                }
                            ))),
                        ),
                        Property::new(SPA_PARAM_BUFFERS_blocks, pod::Value::Int(plane_count)),
                        // Property::new(SPA_PARAM_BUFFERS_size, pod::Value::Int(size as i32)),
                        // Property::new(SPA_PARAM_BUFFERS_stride, pod::Value::Int(stride as i32)),
                        // Property::new(SPA_PARAM_BUFFERS_align, pod::Value::Int(16)),
                        Property::new(
                            SPA_PARAM_BUFFERS_dataType,
                            pod::Value::Choice(ChoiceValue::Int(Choice(
                                ChoiceFlags::empty(),
                                ChoiceEnum::Flags {
                                    default: 1 << DataType::DmaBuf.as_raw(),
                                    flags: vec![1 << DataType::DmaBuf.as_raw()],
                                },
                            ))),
                        ),
                    );

                    // TODO: embedded cursor type

                    let mut b1 = vec![];
                    let mut params = [make_pod(&mut b1, o1)];

                    if let Err(err) = stream.update_params(&mut params) {
                        warn!(?err, "error updating stream params");
                        stop_cast();
                    }
                }
            })
            .add_buffer({
                let stop_cast = stop_cast.clone();
                move |stream, inner, buffer| {
                    crate::profile_scope!("pw_stream::add_buffer", &stream.node_id().to_string());

                    let span = debug_span!("pw_stream");
                    span.record("node_id", stream.node_id());
                    let _guard = span.enter();

                    let mut inner = inner.borrow_mut();
                    let CastState::Ready {
                        size,
                        alpha,
                        modifier,
                        ..
                    } = inner.state
                    else {
                        trace!("add buffer, but cast is not ready yet");
                        return;
                    };

                    debug!(?size, ?alpha, ?modifier, "add_buffer");

                    let spa_buffer = unsafe { &mut (*(*buffer).buffer) };
                    let fourcc = if alpha {
                        Fourcc::Argb8888
                    } else {
                        Fourcc::Xrgb8888
                    };

                    let dmabuf = match allocate_dmabuf(&gbm, size, fourcc, modifier) {
                        Ok(dmabuf) => dmabuf,
                        Err(err) => {
                            warn!("error allocating dmabuf: {err:?}");
                            stop_cast();
                            return;
                        }
                    };

                    let plane_count = dmabuf.num_planes();
                    let spa_datas = unsafe {
                        std::slice::from_raw_parts_mut(
                            spa_buffer.datas,
                            spa_buffer.n_datas as usize,
                        )
                    };
                    assert_eq!(spa_datas.len(), plane_count);

                    for (((fd, stride), offset), spa_data) in dmabuf
                        .handles()
                        .zip(dmabuf.strides())
                        .zip(dmabuf.offsets())
                        .zip(spa_datas.iter_mut())
                    {
                        assert!(spa_data.type_ & (1 << DataType::DmaBuf.as_raw()) > 0);
                        spa_data.type_ = DataType::DmaBuf.as_raw();
                        spa_data.flags = SPA_DATA_FLAG_READWRITE;
                        spa_data.mapoffset = 0;
                        spa_data.fd = fd.as_raw_fd() as i64;
                        spa_data.maxsize = 0;
                        spa_data.data = std::ptr::null_mut();

                        let spa_chunk = unsafe { &mut (*spa_data.chunk) };
                        // clients have implemented to check chunk->size if the buffer is valid
                        // instead of using the flags. Until they are
                        // patched we should use some arbitrary value.
                        spa_chunk.size = 10;
                        spa_chunk.offset = offset;
                        spa_chunk.stride = stride as i32;
                    }

                    let fd = spa_datas[0].fd;
                    assert!(inner.dmabufs.insert(fd, dmabuf).is_none());

                    // During size re-negotiation, the stream sometimes just keeps running, in
                    // which case we may need to force a redraw once we got a newly sized buffer.
                    if inner.dmabufs.len() == 1 && stream.state() == StreamState::Streaming {
                        redraw();
                    }
                }
            })
            .remove_buffer({
                move |stream, inner, buffer| {
                    crate::profile_scope!(
                        "pw_stream::remove_buffer",
                        &stream.node_id().to_string()
                    );

                    let span = debug_span!("pw_stream");
                    span.record("node_id", stream.node_id());
                    let _guard = span.enter();

                    let mut inner = inner.borrow_mut();
                    debug!("remove_buffer");

                    unsafe {
                        let spa_buffer = (*buffer).buffer;
                        let spa_data = (*spa_buffer).datas;
                        assert!((*spa_buffer).n_datas > 0);

                        let fd = (*spa_data).fd;
                        inner.dmabufs.remove(&fd);
                    }
                }
            })
            .register()
            .unwrap();

        let params;
        make_params!(params, &render_formats, size, refresh, alpha);
        stream
            .connect(
                pipewire::spa::utils::Direction::Output,
                None,
                StreamFlags::DRIVER | StreamFlags::ALLOC_BUFFERS,
                params,
            )
            .context("error connecting stream")?;

        self.casts.push(Cast {
            id: cast_id,
            inner,
            stream,
            _listener: listener,
            to_compositor_token,
            session_handle,
            has_alpha: alpha,
            render_formats: render_formats.clone(),
            cursor_mode,
            source,
        });

        Ok(())
    }
}

static CAST_IDS: AtomicUsize = AtomicUsize::new(0);
/// Identifier of a [`Cast`].
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct CastId(usize);
impl CastId {
    /// Create a unique [`WorkspaceId`].
    ///
    /// Panics when you create [`usize::MAX - 1`] items.
    fn unique() -> Self {
        Self(CAST_IDS.fetch_add(1, atomic::Ordering::SeqCst))
    }
}
impl std::fmt::Debug for CastId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cast-{}", self.0)
    }
}
impl std::ops::Deref for CastId {
    type Target = usize;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A single cast stream to for a XDG screencast session.
pub struct Cast {
    /// The unique identifier for this [`Cast`].
    id: CastId,

    /// Shared state between the pipewire main loop and the [`Cast`]
    inner: Rc<RefCell<CastInner>>,

    /// The pipewire [`Stream`] associatedd with this [`Cast`].
    ///
    /// It is our only way to communicate between the compositor and client, in order to inform
    /// it of parameter updates (refresh rate, size, etc...)
    pub stream: Stream,
    /// Listener to the pipewire stream events.
    ///
    /// Dropping this will drop the connection to the [`Stream`].
    _listener: pipewire::stream::StreamListener<Rc<RefCell<CastInner>>>,

    /// The calloop [`RegistrationToken`] that holds the channel listening to events from pipewire
    ///
    /// It should be removed from the event loop when stopping the cast
    pub to_compositor_token: RegistrationToken,

    /// The session handle of the screencast session this [`Cast`] is streaming to.
    pub session_handle: zvariant::OwnedObjectPath,

    /// Whether we offer alpha support for this [`Cast`].
    has_alpha: bool,
    /// The supported render formats for this cast.
    render_formats: FormatSet,

    /// The [`CursorMode`] of this [`Cast`], I.E. how we should display the cursor.
    cursor_mode: CursorMode,
    /// The source that this cast is streaming from.
    source: CastSource,
}

// State that is shared between the pw loop and the cast itself.
struct CastInner {
    /// Whether the [`Cast`] is active.
    active: bool,
    /// The node id of this [`Cast`].
    node_id: Option<u32>,
    /// The DMA buffers shared from the client we are streaming to.
    dmabufs: HashMap<i64, Dmabuf>,
    /// The current refresh rate, in hertz of the [`Cast`].
    ///
    /// Most of the times this is the same as the [`Output`] we are streaming from.
    refresh: u32,
    /// The state of the [`Cast`].
    state: CastState,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CastSource {
    /// The cast is streaming from a downgraded [`Output`].
    Output(WeakOutput),
    /// The cast is streaming from a [`Workspace`].
    Workspace { output: WeakOutput, index: usize },
    /// The cast is streaming from a [`Window`].
    Window(WeakWindow),
}

#[allow(clippy::large_enum_variant)] // we usually jump immediatly to Ready state
enum CastState {
    /// A new size has been sent to the [`Stream`].
    ResizePending { pending_size: Size<u32, Physical> },
    /// When we send new parameters to the [`Stream`], we wait for a params_changed event from the
    /// client to start the rendering process into the provided DMA buffers.
    ConfirmationPending {
        size: Size<u32, Physical>,
        alpha: bool,
        modifier: Modifier,
        plane_count: i32,
    },
    /// The cast is up and running and we are rendering/streaming to the client.
    Ready {
        size: Size<u32, Physical>,
        alpha: bool,
        modifier: Modifier,
        plane_count: i32,
        // damage-tracking initialized when needed.
        damage_tracker: Option<OutputDamageTracker>,
    },
}

impl CastState {
    fn pending_size(&self) -> Option<Size<u32, Physical>> {
        match self {
            Self::ResizePending { pending_size } => Some(*pending_size),
            Self::ConfirmationPending { size, .. } => Some(*size),
            Self::Ready { .. } => None,
        }
    }

    fn expected_format_size(&self) -> Size<u32, Physical> {
        match self {
            Self::ResizePending { pending_size } => *pending_size,
            Self::ConfirmationPending { size, .. } => *size,
            Self::Ready { size, .. } => *size,
        }
    }
}

impl Cast {
    /// Get the unique ID of this [`Cast`].
    pub fn id(&self) -> CastId {
        self.id
    }

    /// Return whether this [`Cast`] is active.
    ///
    /// This is determined by the pipewire [`Stream`] state.
    pub fn active(&self) -> bool {
        self.inner.borrow().active
    }

    /// Get the [`CastSource`] that is [`Cast`] is streaming from.
    pub fn source(&self) -> &CastSource {
        &self.source
    }

    /// Ensure that the cast stream dimensions are `size`.
    ///
    /// If the cast size is already at `size`, return `Ok(true)`.
    /// If a change is needed/pending, this will return `Ok(false)`.
    /// If any error occured while updating stream, return Err(_)
    pub fn ensure_size(&mut self, size: Size<i32, Physical>) -> anyhow::Result<bool> {
        crate::profile_function!();
        let new_size = Size::from((size.w as u32, size.h as u32));

        let mut guard = self.inner.borrow_mut();
        if matches!(&guard.state, CastState::Ready { size, .. } if *size == new_size) {
            return Ok(true);
        }

        if guard.state.pending_size() == Some(new_size) {
            return Ok(false);
        }

        debug!("cast size changed, updating stream size");
        guard.state = CastState::ResizePending {
            pending_size: new_size,
        };

        let params;
        make_params!(
            params,
            &self.render_formats,
            new_size,
            guard.refresh,
            self.has_alpha
        );

        self.stream
            .update_params(params)
            .context("error updating stream params")?;

        Ok(false)
    }

    /// Dequeue the latest stream buffer and render inside of it.
    ///
    /// **NOTE**: This is only meant to be used for an `Output` [`CastSource`]
    ///
    /// Returns `Ok(true)` if we rendered and there was damage.
    /// Returns `Ok(false)` if we rendered and there was NO damage.
    pub fn render_for_output<R: FhtRenderer>(
        &mut self,
        renderer: &mut R,
        output_elements_result: &OutputElementsResult<R>,
        size: Size<i32, Physical>,
        scale: impl Into<Scale<f64>>,
    ) -> anyhow::Result<bool>
    where
        FhtRenderElement<R>: RenderElement<R>,
    {
        crate::profile_function!();

        let elements = if self.cursor_mode.contains(CursorMode::EMBEDDED) {
            &output_elements_result.elements
        } else {
            &output_elements_result.elements[output_elements_result.cursor_elements_len..]
        };

        self.render(renderer, elements, size, scale)
    }

    /// Dequeue the latest stream buffer and render inside of it.
    ///
    /// Returns `Ok(true)` if we rendered and there was damage.
    /// Returns `Ok(false)` if we rendered and there was NO damage.
    pub fn render<R: FhtRenderer>(
        &mut self,
        renderer: &mut R,
        render_elements: &[impl RenderElement<R>],
        size: Size<i32, Physical>,
        scale: impl Into<Scale<f64>>,
    ) -> anyhow::Result<bool> {
        crate::profile_function!();

        let scale = scale.into();
        let mut guard = self.inner.borrow_mut();
        let CastState::Ready { damage_tracker, .. } = &mut guard.state else {
            anyhow::bail!("cast not ready")
        };
        let damage_tracker = damage_tracker
            .get_or_insert_with(|| OutputDamageTracker::new(size, scale, Transform::Normal));

        // Size change will drop the damage tracker, but scale change won't, so check it here.
        let OutputModeSource::Static { scale: t_scale, .. } = damage_tracker.mode() else {
            unreachable!();
        };
        if *t_scale != scale {
            *damage_tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
        }

        let mut buffer = match self.stream.dequeue_buffer() {
            Some(buffer) => buffer,
            None => anyhow::bail!("no available buffer in cast"),
        };

        let fd = buffer.datas_mut()[0].as_raw().fd;
        let mut dmabuf = guard.dmabufs[&fd].clone();

        let damage_tracker = match &mut guard.state {
            CastState::Ready { damage_tracker, .. } => damage_tracker.as_mut().unwrap(),
            _ => unreachable!(),
        };

        let mut fb = renderer.bind(&mut dmabuf)?;
        let res = damage_tracker
            .render_output(
                renderer,
                &mut fb,
                0,
                render_elements,
                smithay::backend::renderer::Color32F::TRANSPARENT,
            )
            .map_err(|err| anyhow::anyhow!("Failed to render inside dmabuf: {err:?}"))?;
        if res.damage.is_none() {
            trace!(cast = ?self.id, "No damage in frame, skipping");
            return Ok(false);
        }
        drop(fb);

        for (data, (stride, offset)) in
            zip(buffer.datas_mut(), zip(dmabuf.strides(), dmabuf.offsets()))
        {
            let chunk = data.chunk_mut();
            *chunk.size_mut() = 1;
            *chunk.stride_mut() = stride as i32;
            *chunk.offset_mut() = offset;

            trace!(
                cast = ?self.id,
                fd = data.as_raw().fd,
                ?stride,
                ?offset,
                "pw_buffer: update"
            );
        }

        Ok(true)
    }
}

fn make_video_object_params(
    size: Size<u32, Physical>,
    formats: &FormatSet,
    refresh: u32,
    alpha: bool,
) -> pod::Object {
    // Present RGB8888 formats since thats the only formats we support from the udev backend.
    // winit backend is not concerned since we dont render screencast on it.
    let (format, fourcc) = if alpha {
        (VideoFormat::BGRA, Fourcc::Argb8888)
    } else {
        (VideoFormat::BGRx, Fourcc::Xrgb8888)
    };
    // Find the format modifiers that match our fourcc
    let supported_modifiers: Vec<_> = formats
        .iter()
        .filter_map(|f| (f.code == fourcc).then_some(u64::from(f.modifier) as i64))
        .collect();

    trace!(?supported_modifiers, "Offering modifiers");

    // If we have more than one modifier, we should not fixate the modifier and run test allocations
    // to find the best one for the GBM device.
    let dont_fixate = if supported_modifiers.len() > 1 {
        PropertyFlags::DONT_FIXATE
    } else {
        PropertyFlags::empty()
    };

    pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pod::property!(FormatProperties::VideoFormat, Id, format),
        Property {
            key: FormatProperties::VideoModifier.as_raw(),
            flags: PropertyFlags::MANDATORY | dont_fixate,
            value: pod::Value::Choice(ChoiceValue::Long(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Enum {
                    default: supported_modifiers[0],
                    alternatives: supported_modifiers,
                }
            )))
        },
        pod::property!(
            FormatProperties::VideoSize,
            Rectangle,
            // Smithay does assertions for us about sizes being always positive.
            // So truncating to u32 does not lose any data.
            Rectangle {
                width: size.w,
                height: size.h,
            }
        ),
        pod::property!(
            FormatProperties::VideoFramerate,
            Fraction,
            Fraction { num: 0, denom: 1 }
        ),
        pod::property!(
            FormatProperties::VideoMaxFramerate,
            Choice,
            Range,
            Fraction,
            // Only support the given refresh rate of the Output.
            Fraction {
                num: refresh,
                denom: 1000
            },
            Fraction { num: 1, denom: 1 },
            Fraction {
                num: refresh,
                denom: 1000
            }
        ),
    )
}

fn make_pod(buffer: &mut Vec<u8>, object: pod::Object) -> &Pod {
    PodSerializer::serialize(Cursor::new(&mut *buffer), &pod::Value::Object(object)).unwrap();
    Pod::from_bytes(buffer).unwrap()
}

// As per pipewire docs, we try to allocate buffers with the given modifier list until we hit a
// successful allocation, in which case that modifier is the correct one to use.
fn find_preferred_modifier(
    gbm: &GbmDevice<DrmDeviceFd>,
    size: Size<u32, Physical>,
    fourcc: Fourcc,
    modifiers: Vec<i64>,
) -> anyhow::Result<(Modifier, usize)> {
    crate::profile_function!();
    debug!(
        ?size,
        ?fourcc,
        ?modifiers,
        "Trying to find preferred modifier"
    );

    let (buffer, modifier) = allocate_buffer(gbm, size, fourcc, &modifiers)?;

    let dmabuf = buffer
        .export()
        .context("error exporting GBM buffer object as dmabuf")?;
    let plane_count = dmabuf.num_planes();

    // FIXME: Ideally this also needs to try binding the dmabuf for rendering.

    Ok((modifier, plane_count))
}

fn allocate_buffer(
    gbm: &GbmDevice<DrmDeviceFd>,
    size: Size<u32, Physical>,
    fourcc: Fourcc,
    modifiers: &[i64],
) -> anyhow::Result<(GbmBuffer, Modifier)> {
    crate::profile_function!();
    let (w, h) = (size.w, size.h);

    if modifiers == [u64::from(Modifier::Invalid) as i64] {
        // modifier-less buffers.
        // The client only provided INVALID modifier with the format.
        let bo = gbm
            .create_buffer_object::<()>(w, h, fourcc, GbmBufferFlags::RENDERING)
            .context("error creating GBM buffer object")?;
        let buffer = GbmBuffer::from_bo(bo, true);
        Ok((buffer, Modifier::Invalid))
    } else {
        // modifier-aware buffers.
        // create_buffer_object_with_modifiers2 will return the best modifier for allocation
        let modifiers = modifiers
            .iter()
            .map(|m| Modifier::from(*m as u64))
            .filter(|m| *m != Modifier::Invalid);
        let bo = gbm
            .create_buffer_object_with_modifiers2::<()>(
                w,
                h,
                fourcc,
                modifiers,
                GbmBufferFlags::RENDERING,
            )
            .context("error creating GBM buffer object")?;

        let modifier = bo.modifier();
        let buffer = GbmBuffer::from_bo(bo, false);
        Ok((buffer, modifier))
    }
}

fn allocate_dmabuf(
    gbm: &GbmDevice<DrmDeviceFd>,
    size: Size<u32, Physical>,
    fourcc: Fourcc,
    modifier: Modifier,
) -> anyhow::Result<Dmabuf> {
    crate::profile_function!();
    let (buffer, _modifier) = allocate_buffer(gbm, size, fourcc, &[u64::from(modifier) as i64])?;
    let dmabuf = buffer
        .export()
        .context("error exporting GBM buffer object as dmabuf")?;
    Ok(dmabuf)
}
