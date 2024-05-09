use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use libc::dev_t;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags};
use smithay::backend::allocator::{Buffer, Fourcc};
use smithay::backend::drm::compositor::{
    DrmCompositor, PrimaryPlaneElement, RenderFrameError, RenderFrameResult,
};
use smithay::backend::drm::gbm::GbmFramebuffer;
use smithay::backend::drm::{
    DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmEventTime, DrmNode, NodeType,
};
use smithay::backend::egl::context::ContextPriority;
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::input::InputEvent;
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::damage::Error as OutputDamageTrackerError;
use smithay::backend::renderer::element::Element;
use smithay::backend::renderer::gles::{Capability, GlesRenderbuffer, GlesRenderer};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::multigpu::gbm::GbmGlesBackend;
use smithay::backend::renderer::multigpu::{
    Error as MultiError, GpuManager, MultiFrame, MultiRenderer,
};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::utils::{CommitCounter, DamageSet};
use smithay::backend::renderer::{
    buffer_type, Bind, Blit, BufferType, ExportMem, Offscreen, TextureFilter,
};
#[cfg(feature = "egl")]
use smithay::backend::renderer::{ImportDma, ImportEgl, ImportMemWl};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{self, UdevBackend, UdevEvent};
use smithay::backend::SwapBuffersError;
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::input::keyboard::XkbConfig;
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{self, Dispatcher, LoopHandle, PostAction, RegistrationToken};
use smithay::reexports::drm::control::connector::{
    self, Handle as ConnectorHandle, Info as ConnectorInfo,
};
use smithay::reexports::drm::control::crtc::Handle as CrtcHandle;
use smithay::reexports::drm::control::ModeTypeFlags;
use smithay::reexports::drm::Device as _;
use smithay::reexports::gbm::{BufferObject, Device as GbmDevice};
use smithay::reexports::input::{DeviceCapability, Libinput};
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{DeviceFd, Monotonic, Point, Rectangle, Time, Transform};
use smithay::wayland::dmabuf::{get_dmabuf, DmabufFeedbackBuilder, DmabufGlobal, ImportNotifier};
use smithay::wayland::drm_lease::{DrmLease, DrmLeaseState};
use smithay::wayland::pointer_gestures::PointerGesturesState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::shm::{self, shm_format_to_fourcc};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use smithay_drm_extras::edid::EdidInfo;

use crate::config::CONFIG;
use crate::renderer::{init_shaders, FhtRenderElement};
use crate::state::{Fht, OutputState, RenderState, State, SurfaceDmabufFeedback};
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
    /// The primary gpu, aka the primary render node.
    pub primary_gpu: DrmNode,
    /// The primary device node, aka the DRM node pointing to your gpu.
    /// It may or may not be the same as the primary_gpu node.
    pub primary_node: DrmNode,
    pub gpu_manager: GpuManager<GbmGlesBackend<GlowRenderer, DrmDeviceFd>>,
    pub devices: HashMap<DrmNode, Device>,
    _registration_tokens: Vec<RegistrationToken>,
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

        let session_token = state
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
                        OutputState::get(output).render_state.queue()
                    }
                }
            })
            .map_err(|_| anyhow::anyhow!("Failed to insert libseat event source!"))?;

        let gpu_manager = GbmGlesBackend::with_factory(|egl_display: &EGLDisplay| {
            let egl_context = EGLContext::new_with_priority(egl_display, ContextPriority::High)?;

            // Thank you cmeissl for guiding me here, this helps with drawing egui since we don't
            // have to create a shadow buffer anymore (atleast if this is false)
            let renderer = if CONFIG.renderer.enable_color_transformations {
                unsafe { GlesRenderer::new(egl_context)? }
            } else {
                let capabilities = unsafe { GlesRenderer::supported_capabilities(&egl_context) }?
                    .into_iter()
                    .filter(|c| *c != Capability::ColorTransformations);
                unsafe { GlesRenderer::with_capabilities(egl_context, capabilities)? }
            };

            Ok(renderer)
        });

        let gpu_manager = GpuManager::new(gpu_manager).expect("Failed to initialize GPU manager!");

        let (primary_gpu, primary_node) =
            if let Some(user_path) = &CONFIG.renderer.render_node.as_ref() {
                let primary_gpu = DrmNode::from_path(user_path)
                    .expect(&format!(
                        "Please make sure that {} is a valid DRM node!",
                        user_path.display()
                    ))
                    .node_with_type(NodeType::Render)
                    .expect("Please make sure that {user_path} is a render node!")
                    .expect("Please make sure that {user_path} is a render node!");
                let primary_node = primary_gpu
                    .node_with_type(NodeType::Primary)
                    .expect("Unable to get primary node from primary gpu node!")
                    .expect("Unable to get primary node from primary gpu node!");

                (primary_gpu, primary_node)
            } else {
                let primary_node = udev::primary_gpu(&seat_name)
                    .unwrap()
                    .and_then(|path| DrmNode::from_path(path).ok())
                    .expect("Failed to get primary gpu!");
                let primary_gpu = primary_node
                    .node_with_type(NodeType::Render)
                    .expect("Failed to get primary gpu node from primary node!")
                    .expect("Failed to get primary gpu node from primary node!");

                (primary_gpu, primary_node)
            };
        info!(?primary_gpu, "Found primary GPU for rendering!");
        info!(?primary_node);

        let mut data = UdevData {
            primary_gpu,
            primary_node,
            gpu_manager,
            session,
            devices: HashMap::new(),
            dmabuf_global: None,
            _registration_tokens: vec![udev_token, session_token, libinput_token],
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

            state.seat = new_seat;
            state.keyboard = keyboard;
            state.pointer = pointer;
        }
        RelativePointerManagerState::new::<State>(&state.display_handle);
        PointerGesturesState::new::<State>(&state.display_handle);

        for (device_id, path) in udev_dispatcher.as_source_ref().device_list() {
            if let Err(err) = data.device_added(device_id, path, state) {
                error!(?err, "Failed to add device!");
            }
        }

        let mut renderer = data.gpu_manager.single_renderer(&primary_gpu).unwrap();
        init_shaders(&mut renderer);

        state.shm_state.update_formats(renderer.shm_formats());

        Ok(data)
    }

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
        if !self.session.is_active() {
            return Ok(());
        }

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

        if device_node == self.primary_node {
            debug!("Adding primary node.");

            let mut renderer = self
                .gpu_manager
                .single_renderer(&render_node)
                .context("Error creating renderer")?;

            #[cfg(feature = "egl")]
            {
                info!(
                    ?self.primary_gpu,
                    "Trying to initialize EGL Hardware Acceleration",
                );
                match renderer.bind_wl_display(&fht.display_handle) {
                    Ok(_) => info!("EGL hardware-acceleration enabled"),
                    Err(err) => info!(?err, "Failed to initialize EGL hardware-acceleration"),
                }
            }

            // Init dmabuf support with format list from our primary gpu
            let dmabuf_formats = renderer.dmabuf_formats().collect::<Vec<_>>();
            let default_feedback = DmabufFeedbackBuilder::new(device_node.dev_id(), dmabuf_formats)
                .build()
                .unwrap();
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
                    surface.dmabuf_feedback = surface.dmabuf_feedback.take().or_else(|| {
                        get_surface_dmabuf_feedback(
                            self.primary_gpu,
                            surface.render_node,
                            &mut self.gpu_manager,
                            &surface.compositor,
                        )
                    });
                });
            });
        }

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

        Ok(())
    }

    /// Remove a device from the backend if found.
    fn device_removed(&mut self, device_id: dev_t, fht: &mut Fht) -> anyhow::Result<()> {
        if !self.session.is_active() {
            return Ok(());
        }

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

            last_primary_swapchain: CommitCounter::default(),
            last_primary_element: CommitCounter::default(),
        };

        device.surfaces.insert(crtc, surface);

        // if let Err(err) = self.schedule_render(&output, Duration::ZERO, &fht.loop_handle) {
        //     error!(?err, "Failed to schedule initial render for surface!");
        // };

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
    pub fn render(
        &mut self,
        fht: &mut Fht,
        output: &Output,
        current_time: Duration,
    ) -> anyhow::Result<bool> {
        let Some((device_node, crtc)) =
            self.devices.iter_mut().find_map(|(device_node, device)| {
                let crtc = device
                    .surfaces
                    .iter()
                    .find(|(_, surface)| surface.output == *output)
                    .map(|(crtc, _)| *crtc);
                crtc.map(|crtc| (*device_node, crtc))
            })
        else {
            anyhow::bail!("No surface matching output!");
        };

        let device = self.devices.get_mut(&device_node).unwrap();
        if !device.drm.is_active() {
            anyhow::bail!("Device DRM is not active!");
        }

        let surface = device.surfaces.get_mut(&crtc).unwrap();

        let Ok(mut renderer) = (if surface.render_node == self.primary_gpu {
            self.gpu_manager.single_renderer(&surface.render_node)
        } else {
            let format = surface.compositor.format();
            self.gpu_manager
                .renderer(&self.primary_gpu, &surface.render_node, format)
        }) else {
            anyhow::bail!("Failed to get renderer!");
        };

        surface.fps.start();

        let output_elements_result =
            fht.output_elements(&mut renderer, &surface.output, &mut surface.fps);
        surface.fps.elements();

        let res = surface
            .compositor
            .render_frame(
                &mut renderer,
                &output_elements_result.render_elements,
                [0.1, 0.1, 0.1, 1.0],
            )
            .map_err(|err| match err {
                RenderFrameError::PrepareFrame(err) => SwapBuffersError::from(err),
                RenderFrameError::RenderFrame(OutputDamageTrackerError::Rendering(err)) => {
                    SwapBuffersError::from(err)
                }
                _ => unreachable!(),
            });
        surface.fps.render();

        match res {
            Err(err) => {
                warn!(?err, "Rendering error!");
            }
            Ok(res) => {
                if res.needs_sync() {
                    if let PrimaryPlaneElement::Swapchain(element) = &res.primary_element {
                        profiling::scope!("SyncPoint::wait");
                        if let Err(err) = element.sync.wait() {
                            error!(?err, "Failed to wait for SyncPoint")
                        };
                    }
                }

                fht.update_primary_scanout_output(output, &res.states);
                if let Some(dmabuf_feedback) = surface.dmabuf_feedback.as_ref() {
                    fht.send_dmabuf_feedbacks(output, dmabuf_feedback, &res.states);
                }

                // wlr-screencopy have to be rendered whether we damaged or not.
                self::render_screencopy(&mut renderer, surface, &res, fht.loop_handle.clone());

                if !res.is_empty {
                    let presentation_feedbacks =
                        fht.take_presentation_feedback(output, &res.states);
                    let data = (presentation_feedbacks, current_time);

                    match surface.compositor.queue_frame(data) {
                        Ok(()) => {
                            let mut output_state = OutputState::get(&surface.output);
                            let new_state = RenderState::WaitingForVblank {
                                redraw_needed: false,
                            };
                            match std::mem::replace(&mut output_state.render_state, new_state) {
                                RenderState::Queued => (),
                                RenderState::WaitingForVblankTimer {
                                    token,
                                    queued: true,
                                } => {
                                    fht.loop_handle.remove(token);
                                }
                                _ => unreachable!(),
                            };

                            // We queued and client buffers are now displayed, we can now send
                            // frame events to them so they start building the next buffer
                            output_state.current_frame_sequence =
                                output_state.current_frame_sequence.wrapping_add(1);
                            // Also notify profiling or our sucess.
                            profiling::finish_frame!();
                            drop(output_state);

                            // Damage also means screencast.
                            #[cfg(feature = "xdg-screencast-portal")]
                            {
                                fht.render_screencast(
                                    output,
                                    &mut renderer,
                                    &output_elements_result,
                                );
                                surface.fps.screencast();
                            }

                            return Ok(true);
                        }
                        Err(err) => {
                            warn!("error queueing frame: {err}");
                        }
                    }
                }
            }
        }

        // We didn't render anything so the output was not damaged, we are still going to try to
        // render after an estimated time till the next Vblank.
        //
        // We use the surface Fps counter to estimate how much time it takes to display a frame, on
        // the last 6 frames, but this could be anything else. (Maybe base this on output refresh?)
        let mut estimated_vblank_duration = surface.fps.avg_frametime();
        if estimated_vblank_duration.is_zero() {
            // In case the average render time is null just use the output's refresh rate and
            // multiply by 0.95 to give some leeway to the compositor to draw.
            let output_refresh = surface.output.current_mode().unwrap().refresh;
            estimated_vblank_duration =
                Duration::from_millis(((1_000_000f32 / output_refresh as f32) * 0.95f32) as u64);
        }

        let mut output_state = OutputState::get(&surface.output);
        match std::mem::take(&mut output_state.render_state) {
            RenderState::Idle => unreachable!(),
            RenderState::Queued => (),
            RenderState::WaitingForVblank { .. } => unreachable!(),
            RenderState::WaitingForVblankTimer { token, .. } => {
                output_state.render_state = RenderState::WaitingForVblankTimer {
                    token,
                    queued: false,
                };
                return Ok(false);
            }
        };

        let timer = Timer::from_duration(estimated_vblank_duration);
        let output = surface.output.clone();
        let token = fht
            .loop_handle
            .insert_source(timer, move |_, _, state| {
                profiling::scope!("estimated vblank timer");
                let mut output_state = OutputState::get(&output);
                output_state.current_frame_sequence =
                    output_state.current_frame_sequence.wrapping_add(1);

                match std::mem::replace(&mut output_state.render_state, RenderState::Idle) {
                    // The timer fired just in front of a redraw.
                    RenderState::WaitingForVblankTimer { queued, .. } => {
                        if queued {
                            output_state.render_state = RenderState::Queued;
                            // If we are queued wait for the next render to send frame callback
                            return TimeoutAction::Drop;
                        }
                    }
                    _ => unreachable!(),
                }

                if output_state.animations_running {
                    output_state.render_state.queue();
                } else {
                    drop(output_state);
                    state.fht.send_frames(&output);
                }

                TimeoutAction::Drop
            })
            .unwrap();
        output_state.render_state = RenderState::WaitingForVblankTimer {
            token,
            queued: false,
        };

        // We did not render anything, but still we queued a next render and so this frame should
        // be considered finished, so profiling should be informed.
        profiling::finish_frame!();

        Ok(false)
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

        match surface
            .compositor
            .frame_submitted()
            .map_err(Into::<SwapBuffersError>::into)
        {
            Ok(user_data) => {
                if let Some((mut feedback, presentation_time)) = user_data {
                    let seq = metadata.sequence;

                    let (clock, flags) = match metadata.time {
                        DrmEventTime::Monotonic(tp) => (
                            tp.into(),
                            wp_presentation_feedback::Kind::Vsync
                                | wp_presentation_feedback::Kind::HwClock
                                | wp_presentation_feedback::Kind::HwCompletion,
                        ),
                        DrmEventTime::Realtime(_) => {
                            (presentation_time, wp_presentation_feedback::Kind::Vsync)
                        }
                    };

                    let refresh = surface
                        .output
                        .current_mode()
                        .map(|mode| Duration::from_secs_f64(1_000f64 / mode.refresh as f64))
                        .unwrap_or_default();

                    feedback.presented::<Time<Monotonic>, _>(
                        clock.into(),
                        refresh,
                        seq as u64,
                        flags,
                    );
                }
            }
            Err(err) => {
                warn!("Error during rendering: {:?}", err);
                match err {
                    SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
                    _ => (),
                }
            }
        };

        let mut output_state = OutputState::get(&surface.output);
        let redraw_needed =
            match std::mem::replace(&mut output_state.render_state, RenderState::Idle) {
                RenderState::WaitingForVblank { redraw_needed } => redraw_needed,
                _ => unreachable!(),
            };

        if redraw_needed || output_state.animations_running {
            output_state.render_state.queue();
        } else {
            drop(output_state);
            fht.send_frames(&surface.output);
        }
    }
}

pub struct Device {
    surfaces: HashMap<CrtcHandle, Surface>,
    pub non_desktop_connectors: Vec<(ConnectorHandle, CrtcHandle)>,
    pub lease_state: Option<DrmLeaseState>,
    pub active_leases: Vec<DrmLease>,
    pub gbm: GbmDevice<DrmDeviceFd>,
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
    // Last primary plane swapchain/element, used to track damage for wlr-screencopy.
    last_primary_swapchain: CommitCounter,
    last_primary_element: CommitCounter,
}

pub type GbmDrmCompositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,
    GbmDevice<DrmDeviceFd>,
    (OutputPresentationFeedback, Duration),
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

/// Render to wlr-screencopy.
///
/// This uses framebuffer blitting instead of rendering with a damage tracker, improving
/// performance. XDG desktop portal is still preferred, but some programs need programmatic copying
/// of the screen (which this provides)
#[profiling::function]
fn render_screencopy<'a>(
    renderer: &mut UdevRenderer<'a>,
    surface: &mut Surface,
    render_frame_result: &RenderFrameResult<
        'a,
        BufferObject<()>,
        GbmFramebuffer,
        FhtRenderElement<UdevRenderer<'a>>,
    >,
    loop_handle: LoopHandle<'static, State>,
) {
    let mut state = OutputState::get(&surface.output);
    let Some(mut screencopy) = state.pending_screencopy.take() else {
        return;
    };
    assert!(screencopy.output() == &surface.output);

    let screencopy_region = screencopy.physical_region();
    let output_size = surface.output.current_mode().unwrap().size;
    let output_scale = surface.output.current_scale().fractional_scale();
    let output_buffer_size = output_size.to_logical(1).to_buffer(1, Transform::Normal);

    // First step: damage the screencopy
    if screencopy.with_damage() {
        if render_frame_result.is_empty {
            // Protocols requires us to not send anything until we get damage.
            state.pending_screencopy.replace(screencopy);
            return;
        };

        let damage = match &render_frame_result.primary_element {
            PrimaryPlaneElement::Swapchain(element) => {
                let swapchain_commit = &mut surface.last_primary_swapchain;
                let damage = element.damage.damage_since(Some(*swapchain_commit));
                *swapchain_commit = element.damage.current_commit();
                damage.map(|dmg| {
                    dmg.into_iter()
                        .map(|rect| {
                            rect.to_logical(1, Transform::Normal, &rect.size)
                                .to_physical(1)
                        })
                        .collect()
                })
            }
            PrimaryPlaneElement::Element(element) => {
                // INFO: Is this element guaranteed to be the same size as the
                // |     output? If not this becomes a
                // FIXME: offset the damage by the element's location
                //
                // also is this even ever reachable?
                let element_commit = &mut surface.last_primary_element;
                let damage = element.damage_since(output_scale.into(), Some(*element_commit));
                *element_commit = element.current_commit();
                Some(damage)
            }
        }
        .unwrap_or_else(|| {
            // Returning `None` means the previous CommitCounter is too old or damage
            // was reset, so damage the whole output
            DamageSet::from_slice(&[Rectangle::from_loc_and_size(
                Point::from((0, 0)),
                output_size,
            )])
        });

        if damage.is_empty() {
            // Still no damage, continue with your day.
            state.pending_screencopy.replace(screencopy);
            return;
        };

        screencopy.damage(&damage);
    }

    // Step 2: Rendering
    let res = if let Ok(dmabuf) = get_dmabuf(screencopy.buffer()) {
        // Make sure everything is inline
        let format_correct =
            Some(dmabuf.format().code) == shm_format_to_fourcc(wl_shm::Format::Argb8888);
        let width_correct = dmabuf.width() == screencopy.physical_region().size.w as u32;
        let height_correct = dmabuf.height() == screencopy.physical_region().size.h as u32;

        if !(format_correct && width_correct && height_correct) {
            return;
        }

        (|| -> anyhow::Result<Option<SyncPoint>> {
            if screencopy_region == Rectangle::from_loc_and_size((0, 0), output_size) {
                renderer.bind(dmabuf)?;
                let blit_frame_result = render_frame_result.blit_frame_result(
                    screencopy_region.size,
                    Transform::Normal,
                    output_scale,
                    renderer,
                    [screencopy_region],
                    [],
                )?;
                Ok(Some(blit_frame_result))
            } else {
                // blit_frame_result can't blit from a specific source rectangle, so blit to an
                // offscreen then to our result.
                let offscreen: GlesRenderbuffer =
                    renderer.create_buffer(Fourcc::Abgr8888, output_buffer_size)?;
                renderer.bind(offscreen.clone())?;

                let sync_point = render_frame_result.blit_frame_result(
                    output_size,
                    Transform::Normal,
                    output_scale,
                    renderer,
                    [Rectangle::from_loc_and_size(Point::default(), output_size)],
                    [],
                )?;

                // NOTE: Doing blit_to offscreen -> dmabuf causes some weird artifacting on the
                // first frames of a wf-recorder recording. But doing so with reversed targets
                // is fine???
                // They both run the same internal code and I don't understand why there's
                // different behaviour. Even adding a missing `self.unbind()?` thats missing from
                // blit_to doesn't fix it.
                renderer.bind(dmabuf)?;
                renderer.blit_from(
                    offscreen,
                    screencopy_region,
                    Rectangle::from_loc_and_size(Point::default(), screencopy_region.size),
                    TextureFilter::Linear,
                )?;

                Ok(Some(sync_point))
            }
        })()
    } else if matches!(buffer_type(screencopy.buffer()), Some(BufferType::Shm)) {
        let res =
            shm::with_buffer_contents_mut(screencopy.buffer(), |shm_ptr, shm_len, buffer_data| {
                // yoinked from pinnacle which
                // yoinked from Niri (thanks yall)
                anyhow::ensure!(
                    // The buffer prefers pixels in little endian ...
                    buffer_data.format == wl_shm::Format::Argb8888
                        && buffer_data.stride == screencopy_region.size.w * 4
                        && buffer_data.height == screencopy_region.size.h
                        && shm_len as i32 == buffer_data.stride * buffer_data.height,
                    "invalid buffer format or size"
                );

                let screencopy_buffer_region = screencopy_region.to_logical(1).to_buffer(
                    1,
                    Transform::Normal,
                    &screencopy_region.size.to_logical(1),
                );

                let offscreen: GlesRenderbuffer =
                    renderer.create_buffer(Fourcc::Abgr8888, output_buffer_size)?;
                renderer.bind(offscreen.clone())?;

                // Blit everything to the offscreen, and then only copy what matters to us.
                // This is for the same reason as above, blit_frame_result cant copy a src
                // rectangle.
                let sync_point = render_frame_result.blit_frame_result(
                    output_size,
                    Transform::Normal,
                    output_scale,
                    renderer,
                    [Rectangle::from_loc_and_size(
                        Point::from((0, 0)),
                        output_size,
                    )],
                    [],
                )?;

                let mapping =
                    renderer.copy_framebuffer(screencopy_buffer_region, Fourcc::Argb8888)?;
                let bytes = renderer.map_texture(&mapping)?;
                anyhow::ensure!(bytes.len() == shm_len, "mapped buffer has wrong length");

                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), shm_ptr, shm_len);
                }

                Ok(Some(sync_point))
            });

        let Ok(res) = res else {
            unreachable!("Buffer is guaranteed to SHM and should be managed by wl_shm!");
        };

        res
    } else {
        Err(anyhow::anyhow!("Unsupported buffer type!"))
    };

    // If i understand everything properly we don't want to submit to the screencopy till the
    // syncpoint is reached, by then the buffer should be filled and we can use it.
    match res {
        Ok(Some(sync_point)) if !sync_point.is_reached() => {
            let Some(sync_fd) = sync_point.export() else {
                screencopy.submit(false);
                return;
            };
            let mut screencopy = Some(screencopy);
            let source = Generic::new(sync_fd, calloop::Interest::READ, calloop::Mode::OneShot);
            let res = loop_handle.insert_source(source, move |_, _, _| {
                let Some(screencopy) = screencopy.take() else {
                    unreachable!("This source is removed after one run");
                };

                screencopy.submit(false);
                Ok(PostAction::Remove)
            });
            if let Err(err) = res {
                error!(?err, "Failed to schedule screencopy submission!");
            }
        }
        Ok(_) => screencopy.submit(false),
        Err(err) => error!(?err, "Failed to submit screencopy!"),
    }
}
