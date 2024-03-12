use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use libc::dev_t;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags};
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::compositor::{DrmCompositor, PrimaryPlaneElement, RenderFrameError};
use smithay::backend::drm::{
    DrmAccessError, DrmDevice, DrmDeviceFd, DrmError, DrmEvent, DrmEventMetadata, DrmEventTime,
    DrmNode, NodeType,
};
use smithay::backend::egl::context::ContextPriority;
use smithay::backend::egl::{EGLDevice, EGLDisplay};
use smithay::backend::input::InputEvent;
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::damage::Error as OutputDamageTrackerError;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::multigpu::gbm::GbmGlesBackend;
use smithay::backend::renderer::multigpu::{
    Error as MultiError, GpuManager, MultiFrame, MultiRenderer,
};
#[cfg(feature = "egl")]
use smithay::backend::renderer::ImportEgl;
use smithay::backend::renderer::{
    self, Bind, BufferType, ExportMem, ImportDma, ImportMemWl, Offscreen,
};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{self, UdevBackend, UdevEvent};
use smithay::backend::SwapBuffersError;
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::input::keyboard::XkbConfig;
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{LoopHandle, RegistrationToken};
use smithay::reexports::drm::control::connector::{
    self, Handle as ConnectorHandle, Info as ConnectorInfo,
};
use smithay::reexports::drm::control::crtc::Handle as CrtcHandle;
use smithay::reexports::drm::control::ModeTypeFlags;
use smithay::reexports::drm::Device as _;
use smithay::reexports::gbm::Device as GbmDevice;
use smithay::reexports::input::{DeviceCapability, Libinput};
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{DeviceFd, Point, Rectangle, Size};
use smithay::wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, ImportNotifier};
use smithay::wayland::drm_lease::{DrmLease, DrmLeaseState};
use smithay::wayland::pointer_gestures::PointerGesturesState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::shm;
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use smithay_drm_extras::edid::EdidInfo;
use wayland_backend::server::GlobalId;

use crate::backend::Backend;
use crate::config::CONFIG;
use crate::handlers::screencopy::PendingScreencopy;
use crate::protocols::screencopy::ScreencopyManagerState;
use crate::shell::decorations::{RoundedOutlineShader, RoundedQuadShader};
use crate::state::{Fht, State, SurfaceDmabufFeedback};
use crate::utils::drm as drm_utils;
use crate::utils::fps::Fps;

// The compositor can't just pick the first format available since some formats even if supported
// make so sense to use since they lose information or are not fun to work with.
//
// Instead we try either 10-bit or 8-bit formats, and on user demand forcibly disable 10-bit
// formats.
const SUPPORTED_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];
const SUPPORTED_FORMATS_8BIT_ONLY: &[Fourcc] = &[Fourcc::Abgr8888, Fourcc::Argb8888];

pub type UdevRenderer<'a> = MultiRenderer<
    'a,
    'a,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
>;

pub type UdevFrame<'a, 'frame> = MultiFrame<
    'a,
    'a,
    'frame,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
>;

pub type UdevRenderError<'a> = MultiError<
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
>;

pub struct UdevData {
    pub session: LibSeatSession,
    dmabuf_global: Option<DmabufGlobal>,
    primary_gpu: DrmNode,
    gpu_manager: GpuManager<GbmGlesBackend<GlowRenderer, DrmDeviceFd>>,
    pub devices: HashMap<DrmNode, Device>,
    registration_tokens: Vec<RegistrationToken>,
}

impl UdevData {
    /// Import this dmabuf buffer to the primary renderer.
    pub fn dmabuf_imported(&mut self, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self
            .gpu_manager
            .single_renderer(&self.primary_gpu)
            .and_then(|mut renderer| renderer.import_dmabuf(&dmabuf, None))
            .is_ok()
        {
            dmabuf.set_node(self.primary_gpu);
            let _ = notifier.successful::<State>();
        } else {
            notifier.failed();
        }
    }

    // Early import this [`WlSurface`] to the [`GpuManager`]
    pub fn early_import(&mut self, surface: &WlSurface) {
        if let Err(err) = self.gpu_manager.early_import(self.primary_gpu, surface) {
            warn!(?err, "Failed to early import buffer!");
        }
    }

    /// Register a device to the udev backend.
    fn device_added(&mut self, device_id: dev_t, path: &Path, fht: &mut Fht) -> anyhow::Result<()> {
        // Get the DRM device from device ID, if any.
        let device_node = DrmNode::from_dev_id(device_id)?;

        // Open the device path with seatd
        let oflags = OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK;
        let fd = self.session.open(path, oflags)?;
        let fd = DrmDeviceFd::new(DeviceFd::from(fd));

        // Create DRM notifier to listen for vblanks.
        let (drm, drm_notifier) = DrmDevice::new(fd.clone(), true)?;

        // Create the GBM device to communicate with the GPU.
        let gbm = GbmDevice::new(fd)?;

        // Listen to DRM events.
        let drm_registration_token = fht
            .loop_handle
            .insert_source(drm_notifier, move |event, metadata, state| match event {
                DrmEvent::VBlank(crtc) => {
                    let metadata = metadata
                        .as_mut()
                        .expect("VBlank events should have metadata!");
                    state
                        .backend
                        .udev()
                        .on_vblank(device_node, crtc, metadata, &mut state.fht);
                }
                DrmEvent::Error(err) => {
                    error!(?err, "Failed to process DRM events!");
                }
            })
            .context("Failed to insert DRM event source!")?;

        // Get the appropriate node for rendering, assuming that the device node is a render node
        // as a fallback if the GBM device doesn't have a render node.
        let render_node =
            EGLDevice::device_for_display(&unsafe { EGLDisplay::new(gbm.clone()).unwrap() })
                .ok()
                .and_then(|x| x.try_get_render_node().ok().flatten())
                .unwrap_or(device_node);

        self.gpu_manager
            .as_mut()
            .add_node(render_node, gbm.clone())
            .context("Failed to add GBM device to GPU manager!")?;

        self.devices.insert(
            device_node,
            Device {
                surfaces: HashMap::new(),
                non_desktop_connectors: Vec::new(),
                lease_state: DrmLeaseState::new::<State>(&fht.display_handle, &device_node)
                    .map_err(|err| {
                        warn!(?err, ?device_node, "Failed to initialize DRM lease state!");
                    })
                    .ok(),
                active_leases: Vec::new(),
                gbm,
                drm,
                drm_scanner: DrmScanner::new(),
                render_node,
                drm_registration_token,
            },
        );

        self.device_changed(device_id, fht)
            .context("Failed to update device!")?;

        Ok(())
    }

    /// Update a device if already registered.
    fn device_changed(&mut self, device_id: dev_t, fht: &mut Fht) -> anyhow::Result<()> {
        let device_node = DrmNode::from_dev_id(device_id)?;
        let Some(device) = self.devices.get_mut(&device_node) else {
            warn!(
                ?device_node,
                "Trying to call device_changed on a non-existent device!"
            );
            return Ok(());
        };

        for event in device.drm_scanner.scan_connectors(&device.drm) {
            match event {
                DrmScanEvent::Connected { connector, crtc } => {
                    if let Some(crtc) = crtc {
                        if let Err(err) =
                            self.connector_connected(device_node, connector, crtc, fht)
                        {
                            error!(?crtc, ?err, "Failed to add connector to device!");
                        };
                    }
                    // No crtc, can't do much for you since I dont even know WHAT you connected.
                }
                DrmScanEvent::Disconnected { connector, crtc } => {
                    if let Some(crtc) = crtc {
                        if let Err(err) =
                            self.connector_disconnected(device_node, connector, crtc, fht)
                        {
                            error!(?crtc, ?err, "Failed to remove connector from device!");
                        }
                    }
                    // No crtc, can't do much for you since I dont even know WHAT you disconnected.
                }
            }
        }

        fht.arrange();

        Ok(())
    }

    /// Remove a device from the backend if found.
    fn device_removed(&mut self, device_id: dev_t, fht: &mut Fht) -> anyhow::Result<()> {
        let device_node = DrmNode::from_dev_id(device_id)?;
        let Some(mut device) = self.devices.remove(&device_node) else {
            warn!(
                ?device_node,
                "Attempted to call device_removed on a non-existent device!"
            );
            return Ok(());
        };

        // Disable every surface.
        let crtcs: Vec<_> = device
            .drm_scanner
            .crtcs()
            .map(|(info, crtc)| (info.clone(), crtc))
            .collect();
        for (connector, crtc) in crtcs {
            let _ = self.connector_disconnected(device_node, connector, crtc, fht);
        }

        // Disable globals
        if let Some(mut leasing_state) = device.lease_state.take() {
            leasing_state.disable_global::<State>();
        }

        self.gpu_manager.as_mut().remove_node(&device.render_node);
        fht.loop_handle.remove(device.drm_registration_token);

        fht.arrange();

        Ok(())
    }

    /// Connect a new CRTC connector.
    ///
    /// This handles creating the output, GBM compositor, and dmabuf globals for this connector.
    fn connector_connected(
        &mut self,
        device_node: DrmNode,
        connector: ConnectorInfo,
        crtc: CrtcHandle,
        fht: &mut Fht,
    ) -> anyhow::Result<()> {
        let Some(device) = self.devices.get_mut(&device_node) else {
            warn!(
                ?device_node,
                "Trying to call connector_connected on a non-existent device!"
            );
            return Ok(());
        };

        let mut renderer = self
            .gpu_manager
            .single_renderer(&device.render_node)
            .unwrap();
        let render_formats = renderer
            .as_mut()
            .egl_context()
            .dmabuf_render_formats()
            .clone();

        let output_name = format!(
            "{}-{}",
            connector.interface().as_str(),
            connector.interface_id()
        );
        info!(?crtc, ?output_name, "Trying to setup connector.");

        let non_desktop =
            match drm_utils::get_property_val(&device.drm, connector.handle(), "non-desktop") {
                Ok((ty, val)) => ty.convert_value(val).as_boolean().unwrap_or(false),
                Err(err) => {
                    warn!(?err, "Assuming connector is meant for desktop.");
                    false
                }
            };

        let (make, model) = EdidInfo::for_connector(&device.drm, connector.handle())
            .map(|info| (info.manufacturer, info.model))
            .unwrap_or_else(|| ("Unknown".into(), "Unknown".into()));

        if non_desktop {
            info!(
                connector_name = output_name,
                "Setting up connector for leasing!"
            );

            device
                .non_desktop_connectors
                .push((connector.handle(), crtc));

            if let Some(leasing_state) = device.lease_state.as_mut() {
                leasing_state.add_connector::<State>(
                    connector.handle(),
                    output_name,
                    format!("{make}-{model}"),
                );
            }

            return Ok(());
        }

        // Get the first preferred mode from the connector mode list, falling back to the first
        // available mode if nothing is preferred (for some obscure reason)
        //
        // TODO: Query output mode from user config and use the one that matches the most
        let drm_mode = *connector
            .modes()
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .unwrap_or_else(|| connector.modes().first().unwrap());
        let mode = OutputMode::from(drm_mode);

        // Create the DRM surface to be associated with the compositor for this surface.
        let surface = device
            .drm
            .create_surface(crtc, drm_mode, &[connector.handle()])
            .context("Failed to create DRM surface for compositor!")?;

        // Create the output object and expose it's wl_output global to clients
        let physical_size = connector
            .size()
            .map(|(w, h)| (w as i32, h as i32))
            .unwrap_or((0, 0))
            .into();
        let physical_properties = PhysicalProperties {
            size: physical_size,
            subpixel: match connector.subpixel() {
                connector::SubPixel::HorizontalRgb => Subpixel::HorizontalRgb,
                connector::SubPixel::HorizontalBgr => Subpixel::HorizontalBgr,
                connector::SubPixel::VerticalRgb => Subpixel::VerticalRgb,
                connector::SubPixel::VerticalBgr => Subpixel::VerticalBgr,
                connector::SubPixel::None => Subpixel::None,
                _ => Subpixel::Unknown,
            },
            make,
            model,
        };
        let output = Output::new(output_name, physical_properties);
        let output_global = output.create_global::<State>(&fht.display_handle);

        output.set_preferred(mode);
        output.change_current_state(Some(mode), None, None, None);
        fht.add_output(output.clone());

        let allocator = GbmAllocator::new(
            device.gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        );

        let color_formats = if CONFIG.renderer.disable_10bit {
            SUPPORTED_FORMATS_8BIT_ONLY
        } else {
            SUPPORTED_FORMATS
        };

        let driver = device
            .drm
            .get_driver()
            .context("Failed to get DRM driver")?;

        let mut planes = surface.planes().clone();

        // Using overlay planes on nvidia GPUs break everything and cause flicker and what other
        // side effects only god knows.
        //
        // I should probably read Nvidia documentation for better info.
        //
        // Just disable them.
        if driver
            .name()
            .to_string_lossy()
            .to_lowercase()
            .contains("nvidia")
            || driver
                .description()
                .to_string_lossy()
                .to_lowercase()
                .contains("nvidia")
        {
            planes.overlay = vec![];
        }

        let compositor = DrmCompositor::new(
            &output,
            surface,
            Some(planes),
            allocator,
            device.gbm.clone(),
            color_formats,
            render_formats,
            device.drm.cursor_size(),
            Some(device.gbm.clone()),
        )
        .context("Failed to create DRM compositor for surface!")?;

        // We only render on one primary gpu, so we don't have to manage different feedbacks based
        // on render nodes.
        let dmabuf_feedback = get_surface_dmabuf_feedback(
            self.primary_gpu,
            device.render_node,
            &mut self.gpu_manager,
            &compositor,
        );

        let surface = Surface {
            render_node: device.render_node,
            output: output.clone(),
            fps: Fps::new(),
            output_global,
            compositor,
            dmabuf_feedback,
        };

        device.surfaces.insert(crtc, surface);

        if let Err(err) = self.schedule_render(&output, Duration::ZERO, &fht.loop_handle) {
            error!(?err, "Failed to schedule initial render for surface!");
        };

        Ok(())
    }

    /// Disconnect this connector, if found.
    fn connector_disconnected(
        &mut self,
        device_node: DrmNode,
        connector: ConnectorInfo,
        crtc: CrtcHandle,
        fht: &mut Fht,
    ) -> anyhow::Result<()> {
        let Some(device) = self.devices.get_mut(&device_node) else {
            warn!(
                ?device_node,
                "Trying to call connector_disconnected on a non-existent device!"
            );
            return Ok(());
        };

        if let Some(pos) = device
            .non_desktop_connectors
            .iter()
            .position(|(handle, _)| *handle == connector.handle())
        {
            // Connector is non-desktop, just disable leasing and unregister it.
            let _ = device.non_desktop_connectors.remove(pos);
            if let Some(leasing_state) = device.lease_state.as_mut() {
                leasing_state.withdraw_connector(connector.handle());
            }
            return Ok(());
        }

        let Some(surface) = device.surfaces.remove(&crtc) else {
            panic!("Tried to remove a non-existant surface!")
        };

        // Remove and disable output.
        let global = surface.output_global;
        fht.display_handle.disable_global::<State>(global.clone());
        let output_clone = surface.output.clone();
        fht.loop_handle
            .insert_source(
                Timer::from_duration(Duration::from_secs(10)),
                move |_time, _, state| {
                    state
                        .fht
                        .display_handle
                        .remove_global::<State>(global.clone());
                    state.fht.remove_output(&output_clone);
                    TimeoutAction::Drop
                },
            )
            .expect("Failed to insert output global removal timer!");

        Ok(())
    }

    /// Request the backend to schedule a next frame for this output.
    #[profiling::function]
    pub fn schedule_render(
        &mut self,
        output: &Output,
        duration: Duration,
        loop_handle: &LoopHandle<'static, State>,
    ) -> anyhow::Result<()> {
        let Some((&device_node, &crtc)) =
            self.devices.iter_mut().find_map(|(device_node, device)| {
                device
                    .surfaces
                    .iter_mut()
                    .find(|(_, s)| s.output == *output)
                    .map(move |(crtc, _)| (device_node, crtc))
            })
        else {
            warn!("Attempted to call schedule_render on a non-existent output!");
            return Ok(());
        };

        loop_handle
            .insert_source(Timer::from_duration(duration), move |_time, _, state| {
                state
                    .backend
                    .udev()
                    .render(device_node, crtc, &mut state.fht);
                TimeoutAction::Drop
            })
            .map_err(|_| anyhow::anyhow!("Failed to insert timer for rendering surface!"))?;

        Ok(())
    }

    /// Render the surface associated with this device and CRTC connector.
    #[profiling::function]
    pub fn render(&mut self, device_node: DrmNode, crtc: CrtcHandle, fht: &mut Fht) {
        let Some(device) = self.devices.get_mut(&device_node) else {
            warn!(
                ?device_node,
                "Attempted to call render on a non-existent device!"
            );
            return;
        };

        let Some(surface) = device.surfaces.get_mut(&crtc) else {
            warn!(
                ?device_node,
                ?crtc,
                "Attempted to call render on a non-existent crtc!"
            );
            return;
        };

        let start = Instant::now();

        fht.advance_animations(&surface.output, fht.clock.now().into());
        let result = render_surface(surface, &mut self.gpu_manager, self.primary_gpu, fht);

        let reschedule = match &result {
            Ok(has_rendered) => !has_rendered,
            Err(err) => {
                warn!("Error during rendering: {:?}", err);
                match err {
                    SwapBuffersError::AlreadySwapped => false,
                    SwapBuffersError::TemporaryFailure(err) => match err.downcast_ref::<DrmError>()
                    {
                        Some(DrmError::DeviceInactive) => true,
                        Some(DrmError::Access(DrmAccessError { source, .. })) => {
                            source.kind() == std::io::ErrorKind::PermissionDenied
                        }
                        _ => false,
                    },
                    SwapBuffersError::ContextLost(err) => match err.downcast_ref::<DrmError>() {
                        Some(DrmError::TestFailed(_)) => {
                            // reset the complete state, disabling all connectors and planes in case
                            // we hit a test failed most likely we hit
                            // this after a tty switch when a foreign master changed CRTC <->
                            // connector bindings and we run in a
                            // mismatch
                            device
                                .drm
                                .reset_state()
                                .expect("failed to reset drm device");
                            true
                        }
                        _ => panic!("Rendering loop lost: {}", err),
                    },
                }
            }
        };

        if reschedule {
            let output_refresh = match surface.output.current_mode() {
                Some(mode) => mode.refresh,
                None => return,
            };
            // If reschedule is true we either hit a temporary failure or more likely rendering
            // did not cause any damage on the output. In this case we just re-schedule a repaint
            // after approx. one frame to re-test for damage.
            let reschedule_duration =
                Duration::from_millis((1_000_000f32 / output_refresh as f32) as u64);
            trace!(
                "reschedule repaint timer with delay {:?} on {:?}",
                reschedule_duration,
                crtc,
            );
            let output = surface.output.clone();
            if let Err(err) = self.schedule_render(&output, reschedule_duration, &fht.loop_handle) {
                warn!(?err, "Failed to reschedule surface!");
            };
        } else {
            let elapsed = start.elapsed();
            tracing::trace!(?elapsed, "rendered surface");
        }

        profiling::finish_frame!();
    }

    /// Handle a DRM VBlank event
    ///
    /// This submits the frame to the comnpositor and schedules a next one if necessary.
    #[profiling::function]
    fn on_vblank(
        &mut self,
        device_node: DrmNode,
        crtc: CrtcHandle,
        metadata: &mut DrmEventMetadata,
        fht: &mut Fht,
    ) {
        let Some(device) = self.devices.get_mut(&device_node) else {
            warn!(
                ?device_node,
                "Attempted to call on_vblank on a non-existent device!"
            );
            return;
        };

        let Some(surface) = device.surfaces.get_mut(&crtc) else {
            warn!(
                ?device_node,
                ?crtc,
                "Attempted to call on_vblank on a non-existent surface!"
            );
            return;
        };

        surface.fps.displayed();

        let should_schedule = match surface
            .compositor
            .frame_submitted()
            .map_err(Into::<SwapBuffersError>::into)
        {
            Ok(user_data) => {
                if let Some(mut feedback) = user_data.flatten() {
                    let seq = metadata.sequence;

                    let (clock, flags) = match metadata.time {
                        DrmEventTime::Monotonic(tp) => (
                            tp.into(),
                            wp_presentation_feedback::Kind::Vsync
                                | wp_presentation_feedback::Kind::HwClock
                                | wp_presentation_feedback::Kind::HwCompletion,
                        ),
                        DrmEventTime::Realtime(_) => {
                            (fht.clock.now(), wp_presentation_feedback::Kind::Vsync)
                        }
                    };

                    let refresh = surface
                        .output
                        .current_mode()
                        .map(|mode| Duration::from_secs_f64(1_000f64 / mode.refresh as f64))
                        .unwrap_or_default();

                    feedback.presented(clock, refresh, seq as u64, flags);
                }

                true
            }
            Err(err) => {
                warn!("Error during rendering: {:?}", err);
                match err {
                    SwapBuffersError::AlreadySwapped => true,
                    // If the device has been deactivated do not reschedule, this will be done
                    // by session resume
                    SwapBuffersError::TemporaryFailure(err)
                        if matches!(
                            err.downcast_ref::<DrmError>(),
                            Some(&DrmError::DeviceInactive)
                        ) =>
                    {
                        false
                    }
                    SwapBuffersError::TemporaryFailure(err) => matches!(
                        err.downcast_ref::<DrmError>(),
                        Some(DrmError::Access(DrmAccessError {
                            source,
                            ..
                        })) if source.kind() == std::io::ErrorKind::PermissionDenied
                    ),
                    SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
                }
            }
        };

        if !should_schedule {
            return;
        }

        // What are we trying to solve by introducing a delay here:
        //
        // Basically it is all about latency of client provided buffers.
        // A client driven by frame callbacks will wait for a frame callback
        // to repaint and submit a new buffer. As we send frame callbacks
        // as part of the repaint in the compositor the latency would always
        // be approx. 2 frames. By introducing a delay before we repaint in
        // the compositor we can reduce the latency to approx. 1 frame + the
        // remaining duration from the repaint to the next VBlank.
        //
        // With the delay it is also possible to further reduce latency if
        // the client is driven by presentation feedback. As the presentation
        // feedback is directly sent after a VBlank the client can submit a
        // new buffer during the repaint delay that can hit the very next
        // VBlank, thus reducing the potential latency to below one frame.
        //
        // Choosing a good delay is a topic on its own so we just implement
        // a simple strategy here. We just split the duration between two
        // VBlanks into two steps, one for the client repaint and one for the
        // compositor repaint. Theoretically the repaint in the compositor should
        // be faster so we give the client a bit more time to repaint. On a typical
        // modern system the repaint in the compositor should not take more than 2ms
        // so this should be safe for refresh rates up to at least 120 Hz. For 120 Hz
        // this results in approx. 3.33ms time for repainting in the compositor.
        // A too big delay could result in missing the next VBlank in the compositor.
        //
        // A more complete solution could work on a sliding window analyzing past repaints
        // and do some prediction for the next repaint.
        let duration = if self.primary_gpu != surface.render_node {
            // However, if we need to do a copy, that might not be enough.
            // (And without actual comparision to previous frames we cannot really know.)
            // So lets ignore that in those cases to avoid thrashing performance.
            trace!("scheduling repaint timer immediately on {:?}", crtc);
            Duration::ZERO
        } else {
            let repaint_delay = surface.fps.avg_rendertime(5);
            trace!(
                "scheduling repaint timer with delay {:?} on {:?}",
                repaint_delay,
                crtc
            );
            repaint_delay
        };

        let output = surface.output.clone();
        if let Err(err) = self.schedule_render(&output, duration, &fht.loop_handle) {
            error!(?err, "Failed to schedule render after VBlank!");
        }
    }
}

/// Render a surface.
///
/// This uses the surface render_node, falling back the primary gpu if necessary.
#[profiling::function]
fn render_surface(
    surface: &mut Surface,
    gpu_manager: &mut GpuManager<GbmGlesBackend<GlowRenderer, DrmDeviceFd>>,
    primary_gpu: DrmNode,
    fht: &mut Fht,
) -> Result<bool, SwapBuffersError> {
    let mut renderer = if surface.render_node == primary_gpu {
        gpu_manager.single_renderer(&surface.render_node)
    } else {
        let format = surface.compositor.format();
        gpu_manager.renderer(&primary_gpu, &surface.render_node, format)
    }
    .unwrap();

    surface.fps.start();

    let elements =
        super::render::output_elements(&mut renderer, &surface.output, fht, &mut surface.fps);
    surface.fps.elements();

    let res = surface
        .compositor
        .render_frame(&mut renderer, &elements, [0.1, 0.1, 0.1, 1.0])
        .map_err(|err| match err {
            RenderFrameError::PrepareFrame(err) => SwapBuffersError::from(err),
            RenderFrameError::RenderFrame(OutputDamageTrackerError::Rendering(err)) => {
                SwapBuffersError::from(err)
            }
            _ => unreachable!(),
        })?;
    surface.fps.render();

    if let Some(mut screencopy) = surface
        .output
        .user_data()
        .get::<PendingScreencopy>()
        .and_then(|scpy| scpy.borrow_mut().take())
    {
        profiling::scope!("PendingScreencopy");
        // Mark entire buffer as damaged.
        let region = screencopy.region();
        if !res.is_empty {
            screencopy.damage(&[Rectangle::from_loc_and_size((0, 0), region.size)]);
        }

        let shm_buffer = screencopy.buffer();

        // Ignore unknown buffer types.
        let buffer_type = renderer::buffer_type(shm_buffer);
        if !matches!(buffer_type, Some(BufferType::Shm)) {
            warn!("Unsupported buffer type: {:?}", buffer_type);
        } else {
            // Create and bind an offscreen render buffer.
            let buffer_dimensions = renderer::buffer_dimensions(shm_buffer).unwrap();
            let offscreen_buffer = Offscreen::<GlesTexture>::create_buffer(
                &mut renderer,
                Fourcc::Argb8888,
                buffer_dimensions,
            )
            .unwrap();
            renderer.bind(offscreen_buffer).unwrap();

            let output = &screencopy.output;
            let scale = output.current_scale().fractional_scale();
            let output_size = output.current_mode().unwrap().size;
            let transform = output.current_transform();

            // Calculate drawing area after output transform.
            let damage = transform.transform_rect_in(region, &output_size);

            let _ = res
                .blit_frame_result(damage.size, transform, scale, &mut renderer, [damage], [])
                .unwrap();

            let region = Rectangle {
                loc: Point::from((region.loc.x, region.loc.y)),
                size: Size::from((region.size.w, region.size.h)),
            };
            let mapping = renderer.copy_framebuffer(region, Fourcc::Argb8888).unwrap();
            let buffer = renderer.map_texture(&mapping);
            // shm_buffer.
            // Copy offscreen buffer's content to the SHM buffer.
            shm::with_buffer_contents_mut(shm_buffer, |shm_buffer_ptr, shm_len, buffer_data| {
                // Ensure SHM buffer is in an acceptable format.
                if buffer_data.format != wl_shm::Format::Argb8888
                    || buffer_data.stride != region.size.w * 4
                    || buffer_data.height != region.size.h
                    || shm_len as i32 != buffer_data.stride * buffer_data.height
                {
                    error!("Invalid buffer format");
                    return;
                }

                // Copy the offscreen buffer's content to the SHM buffer.
                unsafe { shm_buffer_ptr.copy_from(buffer.unwrap().as_ptr(), shm_len) };
            })
            .unwrap();
        }
        // Mark screencopy frame as successful.
        screencopy.submit();
        surface.fps.screencopy();
    }

    if res.needs_sync() {
        if let PrimaryPlaneElement::Swapchain(element) = res.primary_element {
            profiling::scope!("SyncPoint::wait");
            element.sync.wait();
        }
    }

    fht.send_frames(
        &surface.output,
        &res.states,
        surface.dmabuf_feedback.clone(),
    );

    if !res.is_empty {
        let output_presentation_feedback =
            fht.take_presentation_feedback(&surface.output, &res.states);
        surface
            .compositor
            .queue_frame(Some(output_presentation_feedback))
            .map_err(Into::<SwapBuffersError>::into)?;
    }

    Ok(!res.is_empty)
}

/// Initiate the backend
pub fn init(state: &mut State) -> anyhow::Result<()> {
    // Intialize a session with using libseat to communicate with the seatd daemon.
    let (session, notifier) = LibSeatSession::new()
        .context("Failed to create a libseat session! Maybe you should check out your system configuration...")?;
    let seat_name = session.seat();

    let primary_gpu = if let Some(user_path) = &CONFIG.renderer.render_node.as_ref() {
        DrmNode::from_path(user_path)
            .expect(&format!(
                "Please make sure that {} is a valid DRM node!",
                user_path.display()
            ))
            .node_with_type(NodeType::Render)
            .expect("Please make sure that {user_path} is a render node!")
            .unwrap()
    } else {
        udev::primary_gpu(&seat_name)
            .unwrap()
            .and_then(|path| {
                DrmNode::from_path(path)
                    .ok()?
                    .node_with_type(NodeType::Render)?
                    .ok()
            })
            .unwrap_or_else(|| {
                udev::all_gpus(&seat_name)
                    .expect("Failed to query all GPUs from system!")
                    .into_iter()
                    .find_map(|path| DrmNode::from_path(path).ok())
                    .expect("No GPU on your system!")
            })
    };
    info!(?primary_gpu, "Found primary GPU for rendering!");

    let gpu_manager = GpuManager::new(GbmGlesBackend::with_context_priority(ContextPriority::High))
        .expect("Failed to initialize GPU manager!");

    let mut data = UdevData {
        primary_gpu,
        gpu_manager,
        session,
        devices: HashMap::new(),
        dmabuf_global: None,
        registration_tokens: vec![],
    };

    // HACK: You want the wl_seat name to be the same as the libseat session name, so, eh...
    // No clients should have connected to us by now, so we just delete and create one ourselves.
    {
        let seat_global = state.fht.seat.global().unwrap();
        state.fht.display_handle.remove_global::<State>(seat_global);

        let mut new_seat = state
            .fht
            .seat_state
            .new_wl_seat(&state.fht.display_handle, &seat_name);

        let keyboard_config = &CONFIG.input.keyboard;
        let res = new_seat.add_keyboard(
            keyboard_config.get_xkb_config(),
            keyboard_config.repeat_delay,
            keyboard_config.repeat_rate,
        );
        let keyboard = match res {
            Ok(k) => k,
            Err(err) => {
                error!(?err, "Failed to add keyboard! Falling back to defaults");
                new_seat
                    .add_keyboard(
                        XkbConfig::default(),
                        keyboard_config.repeat_delay,
                        keyboard_config.repeat_rate,
                    )
                    .expect("The keyboard is not keyboarding")
            }
        };
        let pointer = new_seat.add_pointer();

        state.fht.seat = new_seat;
        state.fht.keyboard = keyboard;
        state.fht.pointer = pointer;
    }
    RelativePointerManagerState::new::<State>(&state.fht.display_handle);
    PointerGesturesState::new::<State>(&state.fht.display_handle);
    ScreencopyManagerState::new::<State>(&state.fht.display_handle);

    let udev_backend =
        UdevBackend::new(&seat_name).context("Failed to initialize Udev backend source!")?;

    // Initialize libinput so we can listen to events.
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
        data.session.clone().into(),
    );
    libinput_context.udev_assign_seat(&seat_name).unwrap();
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    // Insert event sources inside the event loop
    let libinput_token = state
        .fht
        .loop_handle
        .insert_source(libinput_backend, move |mut event, _, state| {
            if let InputEvent::DeviceAdded { device } = &mut event {
                if device.has_capability(DeviceCapability::Keyboard) {
                    let led_state = state.fht.keyboard.led_state();
                    device.led_update(led_state.into());
                }

                let device_config = CONFIG
                    .input
                    .per_device
                    .get(device.name())
                    .or_else(|| CONFIG.input.per_device.get(device.sysname()));
                let mouse_config =
                    device_config.map_or_else(|| &CONFIG.input.mouse, |cfg| &cfg.mouse);
                let keyboard_config =
                    device_config.map_or_else(|| &CONFIG.input.keyboard, |cfg| &cfg.keyboard);
                let disabled = device_config.map_or(false, |cfg| cfg.disable);

                crate::config::apply_libinput_settings(
                    device,
                    mouse_config,
                    keyboard_config,
                    disabled,
                );

                state.fht.devices.push(device.clone());
            } else if let InputEvent::DeviceRemoved { ref device } = event {
                state.fht.devices.retain(|d| d != device);
            }

            state.process_input_event(event);
        })
        .map_err(|_| anyhow::anyhow!("Failed to insert libinput event source!"))?;
    data.registration_tokens.push(libinput_token);

    let session_token = state
        .fht
        .loop_handle
        .insert_source(notifier, move |event, &mut (), state| match event {
            SessionEvent::PauseSession => {
                info!("Pausing session!");
                libinput_context.suspend();

                for device in state.backend.udev().devices.values_mut() {
                    device.drm.pause();
                    device.active_leases.clear();
                    if let Some(leasing_state) = device.lease_state.as_mut() {
                        leasing_state.suspend();
                    }
                }
            }
            SessionEvent::ActivateSession => {
                info!("Resuming session!");

                if let Err(err) = libinput_context.resume() {
                    error!("Failed to resume libinput context: {:?}", err);
                }

                for device in &mut state.backend.udev().devices.values_mut() {
                    // if we do not care about flicking (caused by modesetting) we could just
                    // pass true for disable connectors here. this would make sure our drm
                    // device is in a known state (all connectors and planes disabled).
                    // but for demonstration we choose a more optimistic path by leaving the
                    // state as is and assume it will just work. If this assumption fails
                    // we will try to reset the state when trying to queue a frame.
                    device.drm.activate(false).expect("Failed to activate DRM!");
                    if let Some(leasing_state) = device.lease_state.as_mut() {
                        leasing_state.resume::<State>();
                    }
                    for surface in device.surfaces.values_mut() {
                        if let Err(err) = surface.compositor.reset_state() {
                            warn!("Failed to reset drm surface state: {}", err);
                        }
                    }
                }

                for output in state.fht.outputs() {
                    let _ = state.backend.udev().schedule_render(
                        output,
                        Duration::ZERO,
                        &state.fht.loop_handle,
                    );
                }
            }
        })
        .map_err(|_| anyhow::anyhow!("Failed to insert libseat event source!"))?;
    data.registration_tokens.push(session_token);

    for (device_id, path) in udev_backend.device_list() {
        if let Err(err) = data.device_added(device_id, path, &mut state.fht) {
            error!(?err, "Failed to add device!");
        }
    }

    let mut renderer = data.gpu_manager.single_renderer(&primary_gpu).unwrap();
    RoundedQuadShader::init(&mut renderer);
    RoundedOutlineShader::init(&mut renderer);

    state.fht.shm_state.update_formats(renderer.shm_formats());

    #[cfg(feature = "egl")]
    {
        info!(
            ?primary_gpu,
            "Trying to initialize EGL Hardware Acceleration",
        );
        match renderer.bind_wl_display(&state.fht.display_handle) {
            Ok(_) => info!("EGL hardware-acceleration enabled"),
            Err(err) => info!(?err, "Failed to initialize EGL hardware-acceleration"),
        }
    }

    // Init dmabuf support with format list from our primary gpu
    let dmabuf_formats = renderer.dmabuf_formats().collect::<Vec<_>>();
    let default_feedback = DmabufFeedbackBuilder::new(primary_gpu.dev_id(), dmabuf_formats)
        .build()
        .unwrap();
    let global = state
        .fht
        .dmabuf_state
        .create_global_with_default_feedback::<State>(&state.fht.display_handle, &default_feedback);
    data.dmabuf_global = Some(global);

    let gpu_manager = &mut data.gpu_manager;
    data.devices.values_mut().for_each(|device| {
        // Update the per drm surface dmabuf feedback
        device.surfaces.values_mut().for_each(|surface| {
            surface.dmabuf_feedback = surface.dmabuf_feedback.take().or_else(|| {
                get_surface_dmabuf_feedback(
                    primary_gpu,
                    surface.render_node,
                    gpu_manager,
                    &surface.compositor,
                )
            });
        });
    });

    let udev_token = state
        .fht
        .loop_handle
        .insert_source(udev_backend, move |event, _, state| match event {
            UdevEvent::Added { device_id, path } => {
                if let Err(err) =
                    state
                        .backend
                        .udev()
                        .device_added(device_id, &path, &mut state.fht)
                {
                    error!(?err, "Failed to add device!");
                }
            }
            UdevEvent::Changed { device_id } => {
                if let Err(err) = state
                    .backend
                    .udev()
                    .device_changed(device_id, &mut state.fht)
                {
                    error!(?err, "Failed to update device!");
                }
            }
            UdevEvent::Removed { device_id } => {
                if let Err(err) = state
                    .backend
                    .udev()
                    .device_removed(device_id, &mut state.fht)
                {
                    error!(?err, "Failed to remove device!");
                }
            }
        })
        .unwrap();
    data.registration_tokens.push(udev_token);

    state.backend = Backend::Udev(data);

    Ok(())
}

pub struct Device {
    surfaces: HashMap<CrtcHandle, Surface>,
    pub non_desktop_connectors: Vec<(ConnectorHandle, CrtcHandle)>,
    pub lease_state: Option<DrmLeaseState>,
    pub active_leases: Vec<DrmLease>,
    gbm: GbmDevice<DrmDeviceFd>,
    pub drm: DrmDevice,
    drm_scanner: DrmScanner,
    render_node: DrmNode,
    drm_registration_token: RegistrationToken,
}

pub struct Surface {
    render_node: DrmNode,
    output: Output,
    fps: Fps,
    output_global: GlobalId,
    compositor: GbmDrmCompositor,
    dmabuf_feedback: Option<SurfaceDmabufFeedback>,
}

pub type GbmDrmCompositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,
    GbmDevice<DrmDeviceFd>,
    Option<OutputPresentationFeedback>,
    DrmDeviceFd,
>;

/// Get the surface dmabuf feedback with the primary_gpu and render_node.
fn get_surface_dmabuf_feedback(
    primary_gpu: DrmNode,
    render_node: DrmNode,
    gpu_manager: &mut GpuManager<GbmGlesBackend<GlowRenderer, DrmDeviceFd>>,
    compositor: &GbmDrmCompositor,
) -> Option<SurfaceDmabufFeedback> {
    let primary_formats = gpu_manager
        .single_renderer(&primary_gpu)
        .ok()?
        .dmabuf_formats()
        .collect::<HashSet<_>>();

    let render_formats = gpu_manager
        .single_renderer(&render_node)
        .ok()?
        .dmabuf_formats()
        .collect::<HashSet<_>>();

    let all_render_formats = primary_formats
        .iter()
        .chain(render_formats.iter())
        .copied()
        .collect::<HashSet<_>>();

    let surface = compositor.surface();
    let planes = surface.planes().clone();

    // We limit the scan-out tranche to formats we can also render from
    // so that there is always a fallback render path available in case
    // the supplied buffer can not be scanned out directly
    let planes_formats = planes
        .primary
        .formats
        .into_iter()
        .chain(planes.overlay.into_iter().flat_map(|p| p.formats))
        .collect::<HashSet<_>>()
        .intersection(&all_render_formats)
        .copied()
        .collect::<Vec<_>>();

    let builder = DmabufFeedbackBuilder::new(primary_gpu.dev_id(), primary_formats);
    let render_feedback = builder
        .clone()
        .add_preference_tranche(render_node.dev_id(), None, render_formats.clone())
        .build()
        .unwrap();

    let scanout_feedback = builder
        .add_preference_tranche(
            surface.device_fd().dev_id().unwrap(),
            Some(zwp_linux_dmabuf_feedback_v1::TrancheFlags::Scanout),
            planes_formats,
        )
        .add_preference_tranche(render_node.dev_id(), None, render_formats)
        .build()
        .unwrap();

    Some(SurfaceDmabufFeedback {
        render_feedback,
        scanout_feedback,
    })
}
