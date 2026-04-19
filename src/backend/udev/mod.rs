use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::time::Duration;

mod device;
mod mode;
mod surface;

use anyhow::Context as _;
use fht_compositor_config::VrrMode;
use libc::dev_t;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::gbm::GbmAllocator;
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::compositor::FrameFlags;
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{
    DrmAccessError, DrmDevice, DrmDeviceFd, DrmError, DrmEvent, DrmEventMetadata, DrmEventTime,
    DrmNode, NodeType,
};
use smithay::backend::egl::{EGLDevice, EGLDisplay};
use smithay::backend::input::InputEvent;
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::multigpu::gbm::GbmGlesBackend;
use smithay::backend::renderer::multigpu::{
    Error as MultiError, GpuManager, MultiFrame, MultiRenderer, MultiTexture, MultiTextureMapping,
};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Color32F, ImportDma, ImportEgl, ImportMemWl};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{self, UdevBackend, UdevEvent};
use smithay::backend::SwapBuffersError;
use smithay::input::keyboard::XkbConfig;
use smithay::output::{Mode as OutputMode, Output};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Dispatcher, RegistrationToken};
use smithay::reexports::drm;
use smithay::reexports::drm::control::crtc::Handle as CrtcHandle;
use smithay::reexports::drm::control::{connector, ResourceHandle};
use smithay::reexports::gbm::{BufferObjectFlags, Device as GbmDevice};
use smithay::reexports::input::{DeviceCapability, Libinput};
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{DeviceFd, Scale};
use smithay::wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, ImportNotifier};
use smithay::wayland::drm_lease::DrmLeaseState;
use smithay::wayland::drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjState};
use smithay::wayland::pointer_gestures::PointerGesturesState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;

use crate::frame_clock::FrameClock;
use crate::handlers::gamma_control::GammaControlState;
use crate::output::RedrawState;
use crate::protocols::output_management;
use crate::renderer::{
    AsGlowRenderer, DebugRenderElement, FhtRenderElement, FhtRenderer, OutputElementsResult,
};
use crate::state::{Fht, State};
use crate::utils::get_monotonic_time;

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

pub type UdevFrame<'a, 'frame, 'buffer> = MultiFrame<
    'a,
    'a,
    'frame,
    'buffer,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
>;

pub type UdevRenderError = MultiError<
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
>;

pub type UdevTextureMapping = MultiTextureMapping<
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
>;

impl FhtRenderer for UdevRenderer<'_> {
    type FhtTextureId = MultiTexture;
    type FhtError = UdevRenderError;
    type FhtTextureMapping = UdevTextureMapping;
}

impl AsGlowRenderer for UdevRenderer<'_> {
    fn glow_renderer(&self) -> &GlowRenderer {
        self.as_ref()
    }

    fn glow_renderer_mut(&mut self) -> &mut GlowRenderer {
        self.as_mut()
    }
}

/// The udev session data.
pub struct UdevData {
    // The [`LibSeatSession`] holding the seat data. This is fetched from the `seatd` daemon (or
    // whatever the equivalent is for system.)
    pub session: LibSeatSession,
    dmabuf_global: Option<DmabufGlobal>,
    // The primary GPU (or render node) used todo all drawing operations.
    //
    // The rendering architecture in `fht-compositor` is rather simplistic, with the primary node
    // doing all/most of the work (fetching surfaces from windows, loading them up, and compositing
    // them into a buffer if needed).
    //
    // What happens from here depends on the surface:
    // 1. If the surface render node is the primary_gpu, the content gets sent to the connector and
    //    displayed to the final user, nothing special here.
    // 2. If the surface render node is not the primary_gpu, the content gets copied and composited
    //    back in a buffer owned by that render node, and then displayed to the final user.
    //
    // FIXME: Perhaps a multi-gpu architecture would be nice, where users can pick&choose what
    //        render node suit them best, for example through a window rule.
    pub primary_gpu: DrmNode,
    pub primary_node: DrmNode,
    pub gpu_manager: GpuManager<GbmGlesBackend<GlowRenderer, DrmDeviceFd>>,
    pub devices: HashMap<DrmNode, device::Device>,
    pub syncobj_state: Option<DrmSyncobjState>,
    _registration_tokens: Vec<RegistrationToken>,
    #[allow(dead_code)]
    pub gamma_control_manager_state: GammaControlState,
}

impl UdevData {
    pub fn new(state: &mut Fht) -> anyhow::Result<Self> {
        // Intialize a session with using libseat to communicate with the seatd daemon.
        let (session, notifier) = LibSeatSession::new()
        .context("Failed to create a libseat session! Maybe you should check out your system configuration...")?;
        let seat_name = session.seat();

        let udev_backend = UdevBackend::new(&seat_name).context("Failed to crate Udev backend!")?;
        let udev_dispatcher =
            Dispatcher::new(udev_backend, |event, (), state: &mut State| match event {
                UdevEvent::Added { device_id, path } => {
                    if let Err(err) =
                        state
                            .backend
                            .udev()
                            .device_added(device_id, &path, &mut state.fht)
                    {
                        error!(?err, "Failed to add device")
                    }
                }
                UdevEvent::Changed { device_id } => {
                    if let Err(err) =
                        state
                            .backend
                            .udev()
                            .device_changed(device_id, &mut state.fht, false)
                    {
                        error!(?err, "Failed to update device")
                    }
                }
                UdevEvent::Removed { device_id } => {
                    if let Err(err) = state
                        .backend
                        .udev()
                        .device_removed(device_id, &mut state.fht)
                    {
                        error!(?err, "Failed to remove device")
                    }
                }
            });
        let udev_token = state
            .loop_handle
            .register_dispatcher(udev_dispatcher.clone())
            .unwrap();

        // Initialize libinput so we can listen to events.
        let mut libinput_context = Libinput::new_with_udev::<
            LibinputSessionInterface<LibSeatSession>,
        >(session.clone().into());
        libinput_context.udev_assign_seat(&seat_name).unwrap();
        let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

        // Insert event sources inside the event loop
        let libinput_token = state
            .loop_handle
            .insert_source(libinput_backend, move |mut event, _, state| {
                if let InputEvent::DeviceAdded { device } = &mut event {
                    if device.has_capability(DeviceCapability::Keyboard) {
                        let led_state = state.fht.keyboard.led_state();
                        device.led_update(led_state.into());
                    }

                    state.fht.add_libinput_device(device.clone());
                } else if let InputEvent::DeviceRemoved { ref device } = event {
                    state.fht.devices.retain(|d| d != device);
                }

                state.process_input_event(event);
            })
            .map_err(|_| anyhow::anyhow!("Failed to insert libinput event source!"))?;

        let session_token = state
            .loop_handle
            .insert_source(notifier, move |event, &mut (), state| match event {
                SessionEvent::PauseSession => {
                    debug!("Pausing session");
                    libinput_context.suspend();

                    for device in state.backend.udev().devices.values_mut() {
                        device.pause();
                    }
                }
                SessionEvent::ActivateSession => {
                    debug!("Resuming session");

                    if let Err(err) = libinput_context.resume() {
                        error!(?err, "Failed to resume libinput context");
                    }

                    for device in &mut state.backend.udev().devices.values_mut() {
                        device.activate();
                    }

                    state.fht.idle_notify_activity();
                    state.fht.queue_redraw_all();
                }
            })
            .map_err(|_| anyhow::anyhow!("Failed to insert libseat event source!"))?;

        let gpu_manager = GbmGlesBackend::default();
        let gpu_manager = GpuManager::new(gpu_manager).expect("Failed to initialize GPU manager!");

        let (primary_gpu, primary_node) = if let Some(user_path) = &state.config.debug.render_node {
            let primary_gpu = DrmNode::from_path(user_path)
                .unwrap_or_else(|_| {
                    panic!(
                        "Please make sure that {} is a valid DRM node!",
                        user_path.display()
                    )
                })
                .node_with_type(NodeType::Render)
                .expect("Please make sure that {user_path} is a render node!")
                .expect("Please make sure that {user_path} is a render node!");
            let primary_node = primary_gpu
                .node_with_type(NodeType::Primary)
                .and_then(Result::ok)
                .unwrap_or_else(|| {
                    warn!("Unable to get primary node from primary gpu node! Falling back to primary gpu node.");
                    primary_gpu
                });

            (primary_gpu, primary_node)
        } else {
            let primary_node = udev::primary_gpu(&seat_name)
                .unwrap()
                .and_then(|path| DrmNode::from_path(path).ok())
                .expect("Failed to get primary gpu!");
            let primary_gpu = primary_node
                .node_with_type(NodeType::Render)
                .and_then(Result::ok)
                .unwrap_or_else(|| {
                    warn!("Unable to get primary node from primary gpu node! Falling back to primary gpu node.");
                    primary_node
                });

            (primary_gpu, primary_node)
        };
        info!(
            ?primary_gpu,
            ?primary_node,
            "Found primary GPU for rendering!"
        );

        let gamma_control_manager_state = GammaControlState::new::<State>(&state.display_handle);

        let mut data = UdevData {
            primary_gpu,
            primary_node,
            gpu_manager,
            session,
            devices: HashMap::new(),
            syncobj_state: None,
            dmabuf_global: None,
            _registration_tokens: vec![udev_token, session_token, libinput_token],
            gamma_control_manager_state,
        };

        // HACK: You want the wl_seat name to be the same as the libseat session name, so, eh...
        // No clients should have connected to us by now, so we just delete and create one
        // ourselves.
        {
            let seat_global = state.seat.global().unwrap();
            state.display_handle.remove_global::<State>(seat_global);

            let mut new_seat = state
                .seat_state
                .new_wl_seat(&state.display_handle, &seat_name);

            let keyboard_config = &state.config.input.keyboard;
            let res = new_seat.add_keyboard(
                keyboard_config.xkb_config(),
                keyboard_config.repeat_delay.get() as i32,
                keyboard_config.repeat_rate.get(),
            );
            let keyboard = match res {
                Ok(k) => k,
                Err(err) => {
                    error!(?err, "Failed to add keyboard! Falling back to defaults");
                    new_seat
                        .add_keyboard(
                            XkbConfig::default(),
                            keyboard_config.repeat_delay.get() as i32,
                            keyboard_config.repeat_rate.get(),
                        )
                        .expect("The keyboard is not keyboarding")
                }
            };
            let pointer = new_seat.add_pointer();

            state.seat = new_seat;
            state.keyboard = keyboard;
            state.pointer = pointer;
        }
        RelativePointerManagerState::new::<State>(&state.display_handle);
        PointerGesturesState::new::<State>(&state.display_handle);

        for (device_id, path) in udev_dispatcher.as_source_ref().device_list() {
            if let Err(err) = data.device_added(device_id, path, state) {
                error!(?err, "Failed to add device")
            }
        }

        let mut renderer = data.gpu_manager.single_renderer(&primary_gpu).unwrap();
        crate::renderer::init(renderer.glow_renderer_mut());

        state.shm_state.update_formats(renderer.shm_formats());

        Ok(data)
    }

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
            warn!(?err, "Failed to early import buffer")
        }
    }

    fn device_added(&mut self, device_id: dev_t, path: &Path, fht: &mut Fht) -> anyhow::Result<()> {
        if !self.session.is_active() {
            return Ok(());
        }

        debug!(?device_id, ?path, "Trying to add DRM device");
        // Get the DRM device from device ID, if any.
        let device_node = DrmNode::from_dev_id(device_id)?;

        // Open the device path with seatd
        let oflags = OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK;
        let device_fd = self.session.open(path, oflags)?;
        let fd = DrmDeviceFd::new(DeviceFd::from(device_fd));

        // Create DRM notifier to listen for vblanks.
        let (drm_device, drm_notifier) = DrmDevice::new(fd.clone(), false)?;

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
                    error!(?device_id, ?err, "Failed to process DRM events")
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

        let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node.into());

        let color_formats = if fht.config.debug.disable_10bit {
            SUPPORTED_FORMATS_8BIT_ONLY
        } else {
            SUPPORTED_FORMATS
        };
        let allocator = GbmAllocator::new(
            gbm.clone(),
            BufferObjectFlags::RENDERING | BufferObjectFlags::SCANOUT,
        );
        let mut renderer = self.gpu_manager.single_renderer(&render_node).unwrap();
        let render_formats = renderer
            .as_mut()
            .egl_context()
            .dmabuf_render_formats()
            .clone();

        let drm_output_manager = DrmOutputManager::new(
            drm_device,
            allocator,
            exporter,
            Some(gbm.clone()),
            color_formats.iter().copied(),
            render_formats,
        );

        if device_node == self.primary_node {
            debug!("Adding primary node");

            let mut renderer = self
                .gpu_manager
                .single_renderer(&render_node)
                .context("Error creating renderer")?;

            match renderer.bind_wl_display(&fht.display_handle) {
                Ok(_) => info!(
                    ?self.primary_gpu,
                    "EGL hardware-acceleration enabled"
                ),
                Err(err) => warn!(?err, "Failed to initialize EGL hardware-acceleration"),
            }

            // Init dmabuf support with format list from our primary gpu
            let dmabuf_formats = renderer.dmabuf_formats();
            let default_feedback = DmabufFeedbackBuilder::new(device_node.dev_id(), dmabuf_formats)
                .build()
                .context("Failed to create dmabuf feedback")?;
            let global = fht
                .dmabuf_state
                .create_global_with_default_feedback::<State>(
                    &fht.display_handle,
                    &default_feedback,
                );
            assert!(self.dmabuf_global.replace(global).is_none());

            self.devices.values_mut().for_each(|device| {
                // Update the per drm surface dmabuf feedback
                device.surfaces.values_mut().for_each(|surface| {
                    if let Err(err) =
                        surface.init_dmabuf_feedback(self.primary_gpu, &mut self.gpu_manager)
                    {
                        warn!(?err, "Failed to initialize surface dmabuf feedback");
                    }
                });
            });

            let import_device = drm_output_manager.device().device_fd().clone();
            if supports_syncobj_eventfd(&import_device) {
                let syncobj_state =
                    DrmSyncobjState::new::<State>(&fht.display_handle, import_device);
                assert!(self.syncobj_state.replace(syncobj_state).is_none());
            }
        }

        let lease_state = DrmLeaseState::new::<State>(&fht.display_handle, &device_node)
            .map_err(|err| warn!(?err, ?device_node, "Failed to initialize DRM lease state"))
            .ok();

        let device = device::Device::new(
            device_node.clone(),
            lease_state,
            drm_output_manager,
            gbm,
            render_node,
            drm_registration_token,
        );

        self.devices.insert(device_node, device);
        self.device_changed(device_id, fht, true)
            .context("Failed to update device!")?;

        Ok(())
    }

    fn device_changed(
        &mut self,
        device_id: dev_t,
        fht: &mut Fht,
        cleanup: bool,
    ) -> anyhow::Result<()> {
        if !self.session.is_active() {
            return Ok(());
        }

        let device_node = DrmNode::from_dev_id(device_id)?;
        let Some(device) = self.devices.get_mut(&device_node) else {
            warn!(
                ?device_node,
                "Trying to call device_changed on a non-existent device!"
            );
            return Ok(());
        };

        device.scan_connectors(fht, &mut self.gpu_manager, cleanup)?;
        // Calling this function will connect any new connectors detected by the device.
        //
        // Disconnected ones should have been handled by scan_connectors, but this still they will
        // also be handled here.
        self.reload_output_configuration(fht, false);

        Ok(())
    }

    fn device_removed(&mut self, device_id: dev_t, fht: &mut Fht) -> anyhow::Result<()> {
        if !self.session.is_active() {
            return Ok(());
        }

        let device_node = DrmNode::from_dev_id(device_id)?;
        let Some(device) = self.devices.remove(&device_node) else {
            warn!(
                ?device_node,
                "Attempted to call device_removed on a non-existent device!"
            );
            return Ok(());
        };

        device.remove(fht, &mut self.gpu_manager)
    }

    pub fn render(
        &mut self,
        fht: &mut Fht,
        output: &Output,
        target_presentation_time: Duration,
    ) -> anyhow::Result<bool> {
        crate::profile_function!();

        let UdevOutputData { device, crtc } = output.user_data().get().unwrap();

        let device = self.devices.get_mut(device).unwrap();
        if !device.is_active() {
            anyhow::bail!("Device DRM is not active")
        }

        let Some(surface) = device.surfaces.get_mut(crtc) else {
            // This can happen if the output got disconnected, but connector_disconneted didn't
            // fire yet, hence Fht::remove_output not triggered.
            error!("Missing surface for output");
            return Ok(false);
        };

        let render_node = surface.render_node();

        let renderer = if render_node == self.primary_gpu {
            self.gpu_manager.single_renderer(&render_node)
        } else {
            let copy_format = surface.copy_format();
            self.gpu_manager
                .renderer(&self.primary_gpu, &render_node, copy_format)
        };

        let mut renderer = renderer.context("Failed to get surface renderer")?;
        let mut output_elements_result = fht.output_elements(&mut renderer, output);

        // To render damage we just use solid color elements,
        if fht.config.debug.draw_damage {
            let state = fht.output_state.get_mut(output).unwrap();
            draw_damage(
                output,
                &mut state.debug_damage_tracker,
                &mut output_elements_result.elements,
            );
        }

        if fht.config.debug.draw_opaque_regions {
            let scale = output.current_scale().integer_scale() as f64;
            draw_opaque_regions(&mut output_elements_result.elements, scale.into());
        }

        // Renderand check for damage.
        let res = surface.render(
            &mut renderer,
            &output_elements_result,
            target_presentation_time,
            FrameFlags::DEFAULT,
            fht,
        );

        match res {
            Err(err) => {
                warn!(?err, "Rendering error");
                // anyhow::bail!() -> don't reschedule and exit out instead
                match err {
                    SwapBuffersError::AlreadySwapped => anyhow::bail!("Already swapped"),
                    SwapBuffersError::TemporaryFailure(err) => match err.downcast_ref::<DrmError>()
                    {
                        Some(DrmError::DeviceInactive) => (),
                        Some(DrmError::Access(DrmAccessError { source, .. }))
                            if source.kind() != io::ErrorKind::PermissionDenied => {}
                        _ => anyhow::bail!("temporary render failure: {err:?}"),
                    },
                    SwapBuffersError::ContextLost(err) => match err.downcast_ref::<DrmError>() {
                        Some(DrmError::TestFailed(_)) => {
                            // reset the complete state, disabling all connectors and planes in case
                            // we hit a test failed most likely we hit
                            // this after a tty switch when a foreign master changed CRTC <->
                            // connector bindings and we run in a
                            // mismatch
                            device
                                .drm_output_manager
                                .device_mut()
                                .reset_state()
                                .expect("failed to reset drm device");
                        }
                        _ => panic!("Rendering loop lost: {}", err),
                    },
                };
            }
            Ok(true) => return Ok(true), // we renderered, handle back the wayland side.
            Ok(false) => (),             // see below
        }

        // Submitted buffers but there was no damage.
        // Send frame callbacks after approximated time
        let output_state = fht.output_state.get_mut(output).unwrap();
        match std::mem::take(&mut output_state.redraw_state) {
            RedrawState::Idle => unreachable!(),
            RedrawState::Queued => (),
            RedrawState::WaitingForVblank { .. } => unreachable!(),
            RedrawState::WaitingForEstimatedVblankTimer { token, .. } => {
                output_state.redraw_state = RedrawState::WaitingForEstimatedVblankTimer {
                    token,
                    queued: false,
                };
                return Ok(false);
            }
        };

        let now = get_monotonic_time();
        let mut duration = target_presentation_time.saturating_sub(now);
        if duration.is_zero() {
            // No use setting a zero timer, since we'll send frame callbacks anyway right after the
            // call to render(). This can happen for example with unknown presentation time from
            // DRM.
            duration += output_state
                .frame_clock
                .refresh_interval()
                .expect("udev backend should not have unknown refresh interval");
        }
        trace!(?duration, "starting estimated vblank timer");

        let surface = device.surfaces.get_mut(crtc).unwrap();
        let timer = Timer::from_duration(duration);
        let output = surface.output().clone();
        let token = fht
            .loop_handle
            .insert_source(timer, move |_, _, state| {
                crate::profile_scope!("vblank-{name}");
                let output_state = state.fht.output_state.get_mut(&output).unwrap();
                output_state.current_frame_sequence =
                    output_state.current_frame_sequence.wrapping_add(1);

                match std::mem::replace(&mut output_state.redraw_state, RedrawState::Idle) {
                    // The timer fired just in front of a redraw.
                    RedrawState::WaitingForEstimatedVblankTimer { queued, .. } => {
                        if queued {
                            // Just wait for the next redraw call to send frame callbacks
                            output_state.redraw_state = RedrawState::Queued;
                            return TimeoutAction::Drop;
                        }
                    }
                    _ => unreachable!(),
                }

                if output_state.animations_running {
                    output_state.redraw_state.queue();
                } else {
                    state.fht.send_frames(&output);
                }

                TimeoutAction::Drop
            })
            .unwrap();
        output_state.redraw_state = RedrawState::WaitingForEstimatedVblankTimer {
            token,
            queued: false,
        };

        Ok(false)
    }

    fn on_vblank(
        &mut self,
        device_node: DrmNode,
        crtc: CrtcHandle,
        metadata: &mut DrmEventMetadata,
        fht: &mut Fht,
    ) {
        crate::profile_function!();

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

        let output_state = fht.output_state.get_mut(surface.output()).unwrap();
        let redraw_queued =
            match std::mem::replace(&mut output_state.redraw_state, RedrawState::Idle) {
                RedrawState::WaitingForVblank { queued } => queued,
                _ => unreachable!(),
            };

        let now = get_monotonic_time();
        let presentation_time = match metadata.time {
            DrmEventTime::Monotonic(tp) => tp,
            DrmEventTime::Realtime(_) => now,
        };

        if let Err(err) = surface.submit(
            output_state
                .frame_clock
                .refresh_interval()
                .unwrap_or(Duration::ZERO),
            now,
            presentation_time,
            metadata.sequence as u64,
        ) {
            warn!(?err, "Failed to submit frame");
        }

        // Now update the frameclock
        output_state.frame_clock.present(presentation_time);

        if redraw_queued || output_state.animations_running {
            fht.queue_redraw(&surface.output());
        } else {
            fht.send_frames(&surface.output());
        }
    }

    pub fn switch_vt(&mut self, vt_num: i32) {
        self.devices.values_mut().for_each(|device| {
            // FIX: Reset overlay planes when changing VTs since some compositors
            // don't use then and as a result don't clean them.
            let _ = device.reset();
            for surface in device.surfaces.values_mut() {
                let _ = surface.reset(false);
            }
        });

        if let Err(err) = self.session.change_vt(vt_num) {
            error!(?err, "Failed to switch virtual terminals")
        }
    }

    /// Get the GBM device associated with the primary node.
    #[cfg(feature = "xdg-screencast-portal")]
    pub fn primary_gbm_device(&self) -> Option<GbmDevice<DrmDeviceFd>> {
        self.devices
            .get(&self.primary_node)
            .map(device::Device::gbm_device)
    }

    /// Reload output configuration and apply new surface modes.
    pub fn reload_output_configuration(&mut self, fht: &mut Fht, force: bool) {
        crate::profile_function!();

        let mut to_disable = vec![];
        let mut to_enable = vec![];

        for (&node, device) in &mut self.devices {
            for (&crtc, surface) in &mut device.surfaces {
                let output_name = surface.output().name();
                let output_config = fht
                    .config
                    .outputs
                    .get(&output_name)
                    .cloned()
                    .unwrap_or_default();
                let Some(connector) = device.drm_scanner.connectors().get(surface.connector())
                else {
                    error!("Missing connector in DRM scanner");
                    continue;
                };

                let render_node = surface.render_node();
                let renderer = if render_node == self.primary_gpu {
                    self.gpu_manager.single_renderer(&render_node)
                } else {
                    let copy_format = surface.copy_format();
                    self.gpu_manager
                        .renderer(&self.primary_gpu, &render_node, copy_format)
                };

                let Ok(mut renderer) = renderer else {
                    error!("Failed to get surface renderer");
                    continue;
                };

                if output_config.disable {
                    fht.output_management_manager_state
                        .set_head_enabled::<State>(surface.output(), false);
                    to_disable.push((node, connector.clone(), crtc));
                    continue;
                }
                fht.output_management_manager_state
                    .set_head_enabled::<State>(surface.output(), true);

                // Sometimes DRM connectors can have custom modes.
                // ---
                // The user specifies one, for example 1920x1080@165 and we build a new DrmMode out
                // of this and the connector info. We test it, it works, nice, otherwise, use
                // fallback
                let modes = connector.modes();
                let mut requested_mode = mode::get_default_mode(modes);
                let mut custom_mode = None;

                if let Some((width, height, refresh)) = output_config.mode {
                    requested_mode = mode::get_matching_mode(modes, width, height, refresh)
                        .unwrap_or(requested_mode);
                    custom_mode = mode::get_custom_mode(width, height, refresh);
                }

                let render_elements = generate_output_render_elements(fht, &mut renderer);
                let new_mode = custom_mode.unwrap_or(requested_mode);

                let mode_changed = surface.pending_mode() == new_mode;
                let vrr_enabled = surface.vrr_enabled();
                let mut vrr_changed = false;
                // if we are OnDemand we wait for redraw to update.
                vrr_changed |= output_config.vrr == VrrMode::On && !vrr_enabled;
                vrr_changed |= output_config.vrr == VrrMode::Off && vrr_enabled;

                if !mode_changed && !vrr_changed && !force {
                    continue;
                }

                // First try custom mode
                let mut new_mode = None;
                let mut used_custom = false;
                if let Some(custom_mode) = custom_mode {
                    if let Err(err) = surface.use_mode(custom_mode, &mut renderer, &render_elements)
                    {
                        error!(?err, "Failed to apply custom mode for {output_name}");
                    } else {
                        new_mode = Some(custom_mode);
                        used_custom = true;
                    }
                }

                if !used_custom {
                    if let Err(err) =
                        surface.use_mode(requested_mode, &mut renderer, &render_elements)
                    {
                        error!(
                            ?err,
                            "Failed to apply requested/fallback mode for {output_name}"
                        );
                        continue;
                    } else {
                        new_mode = Some(requested_mode);
                    }
                }

                // SAFETY: If there was any error above we would have either fallbacked or
                // continued to the next iteration.
                let new_mode = new_mode.unwrap();

                let wl_mode = OutputMode::from(new_mode);
                surface
                    .output()
                    .change_current_state(Some(wl_mode), None, None, None);
                let output_state = fht.output_state.get_mut(surface.output()).unwrap();
                let refresh_interval =
                    Duration::from_secs_f64(1_000f64 / mode::calculate_refresh_rate(&new_mode));
                let vrr_enabled = surface.vrr_enabled();
                output_state.frame_clock = FrameClock::new(Some(refresh_interval), vrr_enabled);
                fht.output_resized(surface.output());
            }

            for (connector, crtc) in device.drm_scanner.crtcs() {
                if connector.state() != connector::State::Connected {
                    continue;
                }

                // Do not duplicate
                if device.surfaces.contains_key(&crtc)
                    || device
                        .non_desktop_connectors
                        .contains(&(connector.handle(), crtc))
                {
                    continue;
                }

                let output_name = format!(
                    "{}-{}",
                    connector.interface().as_str(),
                    connector.interface_id()
                );
                let output_config = fht
                    .config
                    .outputs
                    .get(&output_name)
                    .cloned()
                    .unwrap_or_default();
                if !output_config.disable {
                    to_enable.push((node, connector.clone(), crtc));
                }
            }
        }

        for (node, connector, crtc) in to_disable {
            let device = self.devices.get_mut(&node).unwrap();
            if let Err(err) =
                device.remove_connector(crtc, connector.handle(), &mut self.gpu_manager, fht)
            {
                warn!(?node, ?crtc, ?err, "Failed to disable connector");
            }
        }

        for (node, connector, crtc) in to_enable {
            let device = self.devices.get_mut(&node).unwrap();
            if let Err(err) = device.add_connector(
                crtc,
                connector,
                self.primary_gpu,
                &mut self.gpu_manager,
                fht,
            ) {
                warn!(?node, ?crtc, ?err, "Failed to enable connector");
            }
        }

        fht.loop_handle
            .insert_idle(|state| output_management::update(state));
    }

    /// Update the Variable Refresh rate state of an output.
    pub fn update_output_vrr(
        &mut self,
        fht: &mut Fht,
        output: &Output,
        vrr: bool,
    ) -> anyhow::Result<()> {
        crate::profile_function!();

        for device in self.devices.values_mut() {
            for surface in device.surfaces.values_mut() {
                if *surface.output() != *output {
                    continue;
                }

                if let Err(err) = surface.set_vrr_enabled(vrr) {
                    warn!(
                        ?err,
                        ?vrr,
                        output = output.name(),
                        "Failed to update output VRR state"
                    );
                }

                let data = fht.output_state.get_mut(output).unwrap();
                let vrr_enabled = surface.vrr_enabled();
                data.frame_clock.set_vrr(vrr_enabled);
                return Ok(());
            }
        }

        Ok(())
    }

    pub fn vrr_enabled(&self, output: &Output) -> anyhow::Result<bool> {
        for device in self.devices.values() {
            for surface in device.surfaces.values() {
                if *surface.output() != *output {
                    continue;
                }

                return Ok(surface.vrr_enabled());
            }
        }

        anyhow::bail!("No matching output found")
    }

    pub fn enable_outputs(&mut self) {
        // Here we actually do nothing, since a next queued draw will trigger the surface/CRTC
        // to re-enable again.
    }

    pub fn disable_outputs(&mut self) {
        for device in self.devices.values_mut() {
            for surface in device.surfaces.values_mut() {
                if let Err(err) = surface.clear() {
                    warn!(?err, "Error clearing drm surface");
                }
            }
        }
    }

    pub fn gamma_size(&self, output: &Output) -> anyhow::Result<usize> {
        let UdevOutputData { device, crtc } = output
            .user_data()
            .get::<UdevOutputData>()
            .context("Invalid udev output")?;
        let device = self.devices.get(device).context("Device not found")?;
        let surface = device.surfaces.get(crtc).context("Surface not found")?;
        surface.gamma_length()
    }

    pub fn set_gamma(
        &mut self,
        output: &Output,
        r: Vec<u16>,
        g: Vec<u16>,
        b: Vec<u16>,
    ) -> anyhow::Result<()> {
        let name = output.name();
        let len = r.len();
        tracing::info!("Setting gamma on {} with {} entries", name, len);

        let expected = self.gamma_size(output)?;
        if expected != len {
            anyhow::bail!(
                "Gamma LUT size mismatch: expected {}, got {}",
                expected,
                len
            );
        }

        let UdevOutputData { device, crtc } = output
            .user_data()
            .get::<UdevOutputData>()
            .context("Invalid output")?;
        let device = self.devices.get_mut(device).context("Device not found")?;
        let surface = device.surfaces.get_mut(crtc).context("Surface not found")?;

        match surface.set_gamma(r, g, b) {
            Ok(_) => {
                tracing::info!("Gamma applied successfully to {}", name);
                Ok(())
            }
            Err(e) => {
                tracing::error!("Gamma failed: {:?}", e);
                Err(e)
            }
        }
    }
}

struct UdevOutputData {
    device: DrmNode,
    crtc: CrtcHandle,
}

const DAMAGE_COLOR: Color32F = Color32F::new(0.3, 0.0, 0.0, 0.3);
const OPAQUE_REGION_COLOR: Color32F = Color32F::new(0.0, 0.0, 0.3, 0.3);
const SEMITRANSPARENT_COLOR: Color32F = Color32F::new(0.0, 0.3, 0.0, 0.3);

fn draw_damage<R: FhtRenderer>(
    output: &Output,
    dt: &mut Option<OutputDamageTracker>,
    elements: &mut Vec<FhtRenderElement<R>>,
) {
    let dt = dt.get_or_insert_with(|| OutputDamageTracker::from_output(output));
    let Ok((Some(damage), _)) = dt.damage_output(1, elements) else {
        return;
    };

    for damage_rect in damage {
        let damage_element: DebugRenderElement = SolidColorRenderElement::new(
            Id::new(),
            *damage_rect,
            CommitCounter::default(),
            DAMAGE_COLOR,
            Kind::Unspecified,
        )
        .into();
        elements.insert(0, damage_element.into())
    }
}

pub fn draw_opaque_regions<R: FhtRenderer>(
    elements: &mut Vec<FhtRenderElement<R>>,
    scale: Scale<f64>,
) {
    crate::profile_function!();

    let mut i = 0;
    while i < elements.len() {
        let elem = &elements[i];
        i += 1;

        // HACK
        if format!("{elem:?}").contains("ExtraDamage") {
            continue;
        }

        let geo = elem.geometry(scale);
        let mut opaque = elem.opaque_regions(scale).to_vec();

        for rect in &mut opaque {
            rect.loc += geo.loc;
        }

        let semitransparent = geo.subtract_rects(opaque.iter().copied());

        for rect in opaque {
            let color = SolidColorRenderElement::new(
                Id::new(),
                rect,
                CommitCounter::default(),
                OPAQUE_REGION_COLOR,
                Kind::Unspecified,
            );
            elements.insert(
                i - 1,
                FhtRenderElement::Debug(DebugRenderElement::Solid(color)),
            );
            i += 1;
        }

        for rect in semitransparent {
            let color = SolidColorRenderElement::new(
                Id::new(),
                rect,
                CommitCounter::default(),
                SEMITRANSPARENT_COLOR,
                Kind::Unspecified,
            );
            elements.insert(
                i - 1,
                FhtRenderElement::Debug(DebugRenderElement::Solid(color)),
            );
            i += 1;
        }
    }
}

fn get_property_val(
    device: &impl drm::control::Device,
    handle: impl ResourceHandle,
    name: &str,
) -> anyhow::Result<(
    drm::control::property::Handle,
    drm::control::property::ValueType,
    drm::control::property::RawValue,
)> {
    let props = device.get_properties(handle)?;
    let (prop_handles, values) = props.as_props_and_values();
    for (&prop, &val) in prop_handles.iter().zip(values.iter()) {
        let info = device.get_property(prop)?;
        if Some(name) == info.name().to_str().ok() {
            let val_type = info.value_type();
            return Ok((prop, val_type, val));
        }
    }
    anyhow::bail!("No prop found for {}", name)
}

fn generate_output_render_elements<'a>(
    fht: &mut Fht,
    renderer: &mut UdevRenderer<'a>,
) -> DrmOutputRenderElements<UdevRenderer<'a>, FhtRenderElement<UdevRenderer<'a>>> {
    let mut render_elements = DrmOutputRenderElements::new();
    let outputs = fht.space.outputs().cloned().collect::<Vec<_>>();

    for output in outputs {
        let UdevOutputData { crtc, .. } = output.user_data().get().unwrap();
        let OutputElementsResult { elements, .. } = fht.output_elements(renderer, &output);
        render_elements.add_output(crtc, [0.1, 0.1, 0.1, 1.0].into(), elements);
    }

    render_elements
}
