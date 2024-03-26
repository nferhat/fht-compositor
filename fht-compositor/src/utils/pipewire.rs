use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::Cursor;
use std::os::fd::{AsFd, AsRawFd};
use std::rc::Rc;
use std::time::Duration;

use anyhow::Context;
use pipewire::properties::Properties;
use pipewire::spa::buffer::DataType;
use pipewire::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pipewire::spa::param::format_utils::parse_format;
use pipewire::spa::param::video::{VideoFormat, VideoInfoRaw};
use pipewire::spa::param::ParamType;
use pipewire::spa::pod::serialize::PodSerializer;
use pipewire::spa::pod::{self, ChoiceValue, Pod, Property, PropertyFlags};
use pipewire::spa::sys::{
    SPA_PARAM_BUFFERS_align, SPA_PARAM_BUFFERS_blocks, SPA_PARAM_BUFFERS_buffers,
    SPA_PARAM_BUFFERS_dataType, SPA_PARAM_BUFFERS_size, SPA_PARAM_BUFFERS_stride,
    SPA_DATA_FLAG_READWRITE,
};
use pipewire::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Rectangle, SpaTypes};
use pipewire::stream::{Stream, StreamFlags, StreamState};
use smithay::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use smithay::backend::allocator::gbm::GbmDevice;
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::DrmDeviceFd;
use smithay::output::Output;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{self, Interest, LoopHandle, Mode, PostAction};
use smithay::reexports::gbm::{BufferObjectFlags as GbmBufferFlags, Modifier};
use smithay::utils::{Logical, Size};

use super::geometry::SizeExt;
use crate::portals::{ScreenCastRequest, ScreenCastResponse, SessionSource, SourceType};
use crate::state::State;

/// A helper PipeWire instance to manage PipeWire streams.
pub struct PipeWire {
    _context: pipewire::context::Context,
    pub core: pipewire::core::Core,
    pub casts: Vec<Cast>,
}

pub struct Cast {
    pub session_path: zvariant::OwnedObjectPath,
    pub stream: Stream,
    _listener: pipewire::stream::StreamListener<()>,
    pub is_active: Rc<Cell<bool>>,
    pub output: Output,
    pub size: Size<i32, Logical>,
    pub dmabufs: Rc<RefCell<HashMap<i32, Dmabuf>>>,
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
            .error(|id, seq, res, message| {
                warn!(?id, ?seq, ?res, ?message, "PipeWire error!");
            })
            .register();
        std::mem::forget(listener);

        // A cool thing about pipewire's main loop is that it's really a fire descriptor, which
        // means we can use it inside the EventLoop using a generic. Really cool.
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
                profiling::scope!("PipeWire main loop iteration");
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

    #[profiling::function]
    pub fn start_cast(
        &self,
        to_compositor: calloop::channel::Sender<ScreenCastRequest>,
        to_screencast: async_std::channel::Sender<ScreenCastResponse>,
        gbm: GbmDevice<DrmDeviceFd>,
        session_path: zvariant::OwnedObjectPath,
        source: SessionSource,
        source_type: SourceType,
    ) -> anyhow::Result<Cast> {
        let Some(output) = source.output().cloned() else {
            anyhow::bail!("Session source has no output!");
        };
        let Some(rec) = source.rectangle() else {
            anyhow::bail!("Session source has no rectangle!");
        };
        let mode = output.current_mode().unwrap();
        let transform = output.current_transform();
        let size = transform.transform_size(rec.size);

        let refresh = mode.refresh;

        let stream = Stream::new(&self.core, "fht-compositor-screencast", Properties::new())
            .context("Error creating new PipeWire Stream!")?;

        // Like in good old wayland-rs times...
        let is_active = Rc::new(Cell::new(false));
        let node_id = Rc::new(Cell::new(None));
        let dmabufs = Rc::new(RefCell::new(HashMap::new()));

        let stop_cast = {
            let to_compositor = to_compositor.clone();
            let session_path = session_path.clone();
            // cool trick with closure from niri
            move || {
                let session_path = session_path.clone();
                if let Err(err) = to_compositor.send(ScreenCastRequest::StopCast { session_path }) {
                    warn!(?err, "error sending StopCast to compositor");
                }
            }
        };

        let listener = stream
            .add_local_listener_with_user_data(())
            .state_changed({
                let is_active = is_active.clone();
                let stop_cast = stop_cast.clone();
                move |stream, (), old, new| {
                    debug!(?old, ?new, "New PipeWire stream state");

                    match new {
                        StreamState::Streaming => {
                            is_active.set(true);
                        }
                        StreamState::Paused => {
                            if node_id.get().is_none() {
                                node_id.set(Some(stream.node_id()));
                                let node_id = node_id.get().unwrap();

                                // We didn't have a node ID yet, send it now.
                                if let Err(err) = to_screencast.send_blocking(
                                    ScreenCastResponse::PipeWireStreamData {
                                        node_id,
                                        location: (rec.loc.x, rec.loc.y),
                                        size: (rec.size.w, rec.size.h),
                                        source_type: source_type.bits(),
                                    },
                                ) {
                                    error!(
                                        ?err,
                                        "Failed to send PipeWire stream data to screencast!"
                                    );
                                    stop_cast();
                                }
                            }
                        }
                        _ => is_active.set(false),
                    }
                }
            })
            .param_changed({
                move |stream, (), id, pod| {
                    let id = ParamType::from_raw(id);
                    debug!(?id, "PipeWire stream paramters changed.");

                    if id != ParamType::Format {
                        return;
                    }

                    let Some(pod) = pod else { return };

                    let (m_type, m_subtype) = match parse_format(pod) {
                        Ok(x) => x,
                        Err(err) => {
                            warn!("pw stream: error parsing format: {err:?}");
                            return;
                        }
                    };

                    if m_type != MediaType::Video || m_subtype != MediaSubtype::Raw {
                        return;
                    }

                    let mut format = VideoInfoRaw::new();
                    format.parse(pod).unwrap();
                    trace!("pw stream: got format = {format:?}");

                    const BPP: u32 = 4;
                    let stride = format.size().width * BPP;
                    let size = stride * format.size().height;

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
                        Property::new(SPA_PARAM_BUFFERS_blocks, pod::Value::Int(1)),
                        Property::new(SPA_PARAM_BUFFERS_size, pod::Value::Int(size as i32)),
                        Property::new(SPA_PARAM_BUFFERS_stride, pod::Value::Int(stride as i32)),
                        Property::new(SPA_PARAM_BUFFERS_align, pod::Value::Int(16)),
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

                    let values = PodSerializer::serialize(
                        Cursor::new(Vec::with_capacity(1024)),
                        &pod::Value::Object(o1),
                    )
                    .unwrap()
                    .0
                    .into_inner();
                    let pod = Pod::from_bytes(&values).unwrap();

                    stream.update_params(&mut [pod]).unwrap();
                }
            })
            .add_buffer({
                let dmabufs = dmabufs.clone();
                let stop_cast = stop_cast.clone();
                move |_stream, (), buffer| {
                    debug!("New PipeWire buffer.");

                    unsafe {
                        let spa_buffer = (*buffer).buffer;
                        let spa_data = (*spa_buffer).datas;
                        assert!((*spa_buffer).n_datas > 0);
                        assert!((*spa_data).type_ & (1 << DataType::DmaBuf.as_raw()) > 0);

                        let bo = match gbm.create_buffer_object::<()>(
                            size.w as u32,
                            size.h as u32,
                            Fourcc::Xrgb8888,
                            GbmBufferFlags::RENDERING | GbmBufferFlags::LINEAR,
                        ) {
                            Ok(bo) => bo,
                            Err(err) => {
                                warn!("error creating GBM buffer object: {err:?}");
                                stop_cast();
                                return;
                            }
                        };
                        let dmabuf = match bo.export() {
                            Ok(dmabuf) => dmabuf,
                            Err(err) => {
                                warn!("error exporting GBM buffer object as dmabuf: {err:?}");
                                stop_cast();
                                return;
                            }
                        };

                        let fd = dmabuf.handles().next().unwrap().as_raw_fd();

                        (*spa_data).type_ = DataType::DmaBuf.as_raw();
                        (*spa_data).maxsize = dmabuf.strides().next().unwrap() * size.h as u32;
                        (*spa_data).fd = fd as i64;
                        (*spa_data).flags = SPA_DATA_FLAG_READWRITE;

                        assert!(dmabufs.borrow_mut().insert(fd, dmabuf).is_none());
                    }
                }
            })
            .remove_buffer({
                let dmabufs = dmabufs.clone();
                move |_stream, (), buffer| {
                    trace!("pw stream: remove_buffer");

                    unsafe {
                        let spa_buffer = (*buffer).buffer;
                        let spa_data = (*spa_buffer).datas;
                        assert!((*spa_buffer).n_datas > 0);

                        let fd = (*spa_data).fd as i32;
                        dmabufs.borrow_mut().remove(&fd);
                    }
                }
            })
            .register()
            .unwrap();

        let object = pod::object!(
            SpaTypes::ObjectParamFormat,
            ParamType::EnumFormat,
            pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
            pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
            pod::property!(FormatProperties::VideoFormat, Id, VideoFormat::BGRx),
            Property {
                key: FormatProperties::VideoModifier.as_raw(),
                value: pod::Value::Long(u64::from(Modifier::Invalid) as i64),
                flags: PropertyFlags::MANDATORY,
            },
            pod::property!(
                FormatProperties::VideoSize,
                Rectangle,
                Rectangle {
                    // NOTE: Smithay has asserts in the size struct to ensure that a size fields are
                    // always positive, so this cast will not lose any information.
                    width: size.w as u32,
                    height: size.h as u32,
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
                Fraction {
                    num: refresh as u32,
                    denom: 1000
                },
                Fraction { num: 1, denom: 1 },
                Fraction {
                    num: refresh as u32,
                    denom: 1000
                }
            ),
        );

        let values = PodSerializer::serialize(
            Cursor::new(Vec::with_capacity(1024)),
            &pod::Value::Object(object),
        )
        .unwrap()
        .0
        .into_inner();
        let pod = Pod::from_bytes(&values).unwrap();

        stream
            .connect(
                pipewire::spa::utils::Direction::Output,
                None,
                StreamFlags::DRIVER | StreamFlags::ALLOC_BUFFERS,
                &mut [pod],
            )
            .context("error connecting stream")?;

        let cast = Cast {
            session_path,
            stream,
            _listener: listener,
            is_active,
            output,
            size: size.as_logical(),
            dmabufs,
        };
        Ok(cast)
    }
}
