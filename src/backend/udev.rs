use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use libc::dev_t;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::gbm::GbmAllocator;
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::compositor::{DrmCompositor, PrimaryPlaneElement, RenderFrameError};
use smithay::backend::drm::{
    DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmEventTime, DrmNode, NodeType,
};
use smithay::backend::egl::{EGLDevice, EGLDisplay};
use smithay::backend::input::InputEvent;
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::damage::{Error as OutputDamageTrackerError, OutputDamageTracker};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
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
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::input::keyboard::XkbConfig;
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Dispatcher, RegistrationToken};
use smithay::reexports::drm::control::connector::{
    self, Handle as ConnectorHandle, Info as ConnectorInfo,
};
use smithay::reexports::drm::control::crtc::Handle as CrtcHandle;
use smithay::reexports::drm::control::{ModeFlags, ModeTypeFlags, ResourceHandle};
use smithay::reexports::drm::{self, Device as _};
use smithay::reexports::gbm::{BufferObjectFlags, Device as GbmDevice};
use smithay::reexports::input::{DeviceCapability, Libinput};
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{DeviceFd, Monotonic};
use smithay::wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, ImportNotifier};
use smithay::wayland::drm_lease::{DrmLease, DrmLeaseState};
use smithay::wayland::pointer_gestures::PointerGesturesState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay_drm_extras::display_info;
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};

use crate::output::RedrawState;
use crate::renderer::shaders::Shaders;
use crate::renderer::{
    AsGlowFrame, AsGlowRenderer, DebugRenderElement, FhtRenderElement, FhtRenderer,
    OutputElementsResult,
};
use crate::state::{Fht, State, SurfaceDmabufFeedback};
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

pub type UdevFrame<'a, 'frame> = MultiFrame<
    'a,
    'a,
    'frame,
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

impl<'a> FhtRenderer for UdevRenderer<'a> {
    type FhtTextureId = MultiTexture;
    type FhtError = UdevRenderError;
    type FhtTextureMapping = UdevTextureMapping;
}

impl<'a> AsGlowRenderer for UdevRenderer<'a> {
    fn glow_renderer(&self) -> &GlowRenderer {
        self.as_ref()
    }

    fn glow_renderer_mut(&mut self) -> &mut GlowRenderer {
        self.as_mut()
    }
}

impl<'a, 'frame> AsGlowFrame<'frame> for UdevFrame<'a, 'frame> {
    fn glow_frame(&self) -> &GlowFrame<'frame> {
        self.as_ref()
    }

    fn glow_frame_mut(&mut self) -> &mut GlowFrame<'frame> {
        self.as_mut()
    }
}

pub struct UdevData {
    pub session: LibSeatSession,
    dmabuf_global: Option<DmabufGlobal>,
    pub primary_gpu: DrmNode,
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
                        error!(?err, "Failed to add device")
                    }
                }
                UdevEvent::Changed { device_id } => {
                    if let Err(err) = state
                        .backend
                        .udev()
                        .device_changed(device_id, &mut state.fht)
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
                        device.drm.pause();
                        device.active_leases.clear();
                        if let Some(leasing_state) = device.lease_state.as_mut() {
                            leasing_state.suspend();
                        }
                    }
                }
                SessionEvent::ActivateSession => {
                    debug!("Resuming session");

                    if let Err(err) = libinput_context.resume() {
                        error!(?err, "Failed to resume libinput context");
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
                                warn!(?err, "Failed to reset drm surface state");
                            }
                        }
                    }

                    state.fht.queue_redraw_all();
                }
            })
            .map_err(|_| anyhow::anyhow!("Failed to insert libseat event source!"))?;

        let gpu_manager = GbmGlesBackend::default();

        let gpu_manager = GpuManager::new(gpu_manager).expect("Failed to initialize GPU manager!");

        let (primary_gpu, primary_node) = if let Some(user_path) = &state.config.debug.render_node {
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
        info!(
            ?primary_gpu,
            ?primary_node,
            "Found primary GPU for rendering!"
        );

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

            let keyboard_config = &state.config.input.keyboard;
            let res = new_seat.add_keyboard(
                keyboard_config.xkb_config(),
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
                error!(?err, "Failed to add device")
            }
        }

        let mut renderer = data.gpu_manager.single_renderer(&primary_gpu).unwrap();
        Shaders::init(renderer.glow_renderer_mut());

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
                        warn!(?err, ?device_node, "Failed to initialize DRM lease state")
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

        let Ok(result) = device
            .drm_scanner
            .scan_connectors(&device.drm)
            .inspect_err(|err| warn!(?err, ?device_node, "Failed to scan connectors for device"))
        else {
            return Ok(());
        };
        for event in result {
            match event {
                DrmScanEvent::Connected { connector, crtc } => {
                    if let Some(crtc) = crtc {
                        if let Err(err) =
                            self.connector_connected(device_node, connector, crtc, fht)
                        {
                            error!(?crtc, ?err, "Failed to add connector to device")
                        };
                    }
                    // No crtc, can't do much for you since I dont even know WHAT you connected.
                }
                DrmScanEvent::Disconnected { connector, crtc } => {
                    if let Some(crtc) = crtc {
                        if let Err(err) =
                            self.connector_disconnected(device_node, connector, crtc, fht)
                        {
                            error!(?crtc, ?err, "Failed to remove connector from device")
                        }
                    }
                    // No crtc, can't do much for you since I dont even know WHAT you disconnected.
                }
            }
        }

        Ok(())
    }

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

    fn connector_connected(
        &mut self,
        device_node: DrmNode,
        connector: ConnectorInfo,
        crtc: CrtcHandle,
        fht: &mut Fht,
    ) -> anyhow::Result<()> {
        debug!(?device_node, ?crtc, "Connector connected");
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
        debug!(?crtc, ?output_name, "Trying to setup connector");

        let non_desktop = match get_property_val(&device.drm, connector.handle(), "non-desktop") {
            Ok((ty, val)) => ty.convert_value(val).as_boolean().unwrap_or(false),
            Err(err) => {
                warn!(
                    ?crtc,
                    ?err,
                    "Failed to get non-desktop property for connector, defaulting to false."
                );
                false
            }
        };

        let info = display_info::for_connector(&device.drm, connector.handle());
        let make = info
            .as_ref()
            .and_then(|info| info.make())
            .unwrap_or_else(|| "Unknown".into());
        let model = info
            .as_ref()
            .and_then(|info| info.model())
            .unwrap_or_else(|| "Unknown".into());

        if non_desktop {
            debug!(
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

        let output_config = fht.config.outputs.get(&output_name);
        let mut new_scale = None;
        let drm_mode;
        fn get_default_mode(modes: &[drm::control::Mode]) -> drm::control::Mode {
            modes
                .iter()
                .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
                .copied()
                .unwrap_or_else(|| *modes.first().unwrap())
        }

        let mut new_transform = None;

        let modes = connector.modes();
        if let Some(output_config) = fht.config.outputs.get(&output_name) {
            if let Some((w, h, refresh)) = output_config.mode {
                drm_mode = match get_matching_mode(modes, w, h, refresh) {
                    // We found a mode specified by the user, best case scenario
                    Some((mode, false)) => mode,
                    Some((mode, true)) => {
                        warn!("Unable to find matching mode for output {output_name}! Using preferred");
                        mode
                    }
                    None => {
                        warn!("Unable to find matching/preferred mode for {output_name}! Using first available mode");
                        *modes.first().unwrap()
                    }
                }
            } else {
                drm_mode = get_default_mode(modes);
            }

            if let Some(transform) = output_config.transform {
                new_transform = Some(transform.into());
            }

            if let Some(scale) = output_config.scale {
                new_scale = Some(smithay::output::Scale::Integer(scale));
            }
        } else {
            drm_mode = get_default_mode(modes);
        }

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
        for mode in modes {
            output.add_mode(OutputMode::from(*mode)); // adversite all the modes
        }
        output.set_preferred(mode);
        output.change_current_state(Some(mode), new_transform, new_scale, None);

        let output_global = output.create_global::<State>(&fht.display_handle);

        let refresh_interval =
            Duration::from_secs_f64(1_000f64 / calculate_refresh_rate(&drm_mode));
        fht.add_output(output.clone(), Some(refresh_interval));

        let allocator = GbmAllocator::new(
            device.gbm.clone(),
            BufferObjectFlags::RENDERING | BufferObjectFlags::SCANOUT,
        );

        let color_formats = if fht.config.debug.disable_10bit {
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
            debug!(?crtc, "Detected nvidia device, disable overlay planes");
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
            output_global,
            compositor,
            dmabuf_feedback,
        };
        fht.queue_redraw(&surface.output);
        device.surfaces.insert(crtc, surface);

        Ok(())
    }

    fn connector_disconnected(
        &mut self,
        device_node: DrmNode,
        connector: ConnectorInfo,
        crtc: CrtcHandle,
        fht: &mut Fht,
    ) -> anyhow::Result<()> {
        debug!(?device_node, ?crtc, "Connector disconnected");
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

    pub fn render(
        &mut self,
        fht: &mut Fht,
        output: &Output,
        target_presentation_time: Duration,
    ) -> anyhow::Result<bool> {
        crate::profile_function!();

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
            anyhow::bail!("No surface matching output")
        };

        let device = self.devices.get_mut(&device_node).unwrap();
        if !device.drm.is_active() {
            anyhow::bail!("Device DRM is not active")
        }

        let surface = device.surfaces.get_mut(&crtc).unwrap();

        let Ok(mut renderer) = (if surface.render_node == self.primary_gpu {
            self.gpu_manager.single_renderer(&surface.render_node)
        } else {
            let format = surface.compositor.format();
            self.gpu_manager
                .renderer(&self.primary_gpu, &surface.render_node, format)
        }) else {
            anyhow::bail!("Failed to get renderer")
        };

        let output_elements_result = {
            let OutputElementsResult {
                mut elements,
                mut cursor_elements_len,
            } = fht.output_elements(&mut renderer, output);

            // To render damage we just use solid color elements,
            if fht.config.debug.draw_damage {
                let state = fht.output_state.get_mut(output).unwrap();
                let damage_elements =
                    draw_damage(output, &mut state.debug_damage_tracker, &elements);
                // HACK: We dont want damage rectangles inside screencopy.
                cursor_elements_len += damage_elements.len();
                elements = damage_elements.into_iter().chain(elements).collect();
            }

            OutputElementsResult {
                elements,
                cursor_elements_len,
            }
        };

        // Renderand check for damage.
        let res = surface
            .compositor
            .render_frame(
                &mut renderer,
                &output_elements_result.elements,
                [0.1, 0.1, 0.1, 1.0],
            )
            .map_err(|err| match err {
                RenderFrameError::PrepareFrame(err) => SwapBuffersError::from(err),
                RenderFrameError::RenderFrame(OutputDamageTrackerError::Rendering(err)) => {
                    SwapBuffersError::from(err)
                }
                _ => unreachable!(),
            });

        match res {
            Err(err) => {
                warn!(?err, "Rendering error")
            }
            Ok(res) => {
                if res.needs_sync() {
                    if let PrimaryPlaneElement::Swapchain(element) = &res.primary_element {
                        crate::profile_scope!("SyncPoint::wait");
                        if let Err(err) = element.sync.wait() {
                            error!(?err, "Failed to wait for SyncPoint")
                        };
                    }
                }

                fht.update_primary_scanout_output(output, &res.states);
                if let Some(dmabuf_feedback) = surface.dmabuf_feedback.as_ref() {
                    fht.send_dmabuf_feedbacks(output, dmabuf_feedback, &res.states);
                }

                // Without damage = we just care that rendering happened.
                //
                // How we proceed with wlr-screencopy is when a client requests a without damage
                // frame, we queue rendering of the output to satisfy the request on
                // the next dispatch cycle
                fht.render_screencopy_without_damage(
                    output,
                    &mut renderer,
                    &output_elements_result,
                );

                if !res.is_empty {
                    // We have damage to submit, take presentation feedback try to queue the next
                    // frame, this is the only code path where we should send frames to clients that
                    // are displayed on the Surface's output.
                    let presentation_feedback = fht.take_presentation_feedback(output, &res.states);

                    match surface.compositor.queue_frame(presentation_feedback) {
                        Ok(()) => {
                            let output_state = fht.output_state.get_mut(output).unwrap();
                            let new_state = RedrawState::WaitingForVblank { queued: false };
                            match std::mem::replace(&mut output_state.redraw_state, new_state) {
                                RedrawState::Queued => (),
                                RedrawState::WaitingForEstimatedVblankTimer {
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
                            // Also notify puffin of a new frame.
                            #[cfg(feature = "profile-with-puffin")]
                            puffin::GlobalProfiler::lock().new_frame();

                            // Damage also means screencast.
                            #[cfg(feature = "xdg-screencast-portal")]
                            {
                                fht.render_screencast(
                                    output,
                                    &mut renderer,
                                    &output_elements_result,
                                );
                            }
                            // And also screencopy.
                            fht.render_screencopy_with_damage(
                                output,
                                &mut renderer,
                                &output_elements_result,
                            );

                            return Ok(true);
                        }
                        Err(err) => {
                            warn!("error queueing frame: {err}");
                        }
                    }
                }
            }
        }

        // Submitted buffers but there was no damage.
        // Send frame callbacks after approx
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

        let timer = Timer::from_duration(duration);
        let output = surface.output.clone();
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

        let output_state = fht.output_state.get_mut(&surface.output).unwrap();
        let redraw_queued =
            match std::mem::replace(&mut output_state.redraw_state, RedrawState::Idle) {
                RedrawState::WaitingForVblank { queued } => queued,
                _ => unreachable!(),
            };

        let now = get_monotonic_time();
        let presentation_time = match metadata.time {
            DrmEventTime::Monotonic(tp) => tp.into(),
            DrmEventTime::Realtime(_) => now,
        };

        match surface
            .compositor
            .frame_submitted()
            .map_err(Into::<SwapBuffersError>::into)
        {
            Ok(Some(mut presentation_feedback)) => {
                let refresh = output_state
                    .frame_clock
                    .refresh_interval()
                    .unwrap_or(Duration::ZERO);
                // FIXME: ideally should be monotonically increasing for a surface.
                let seq = metadata.sequence as u64;
                let mut flags = wp_presentation_feedback::Kind::Vsync
                    | wp_presentation_feedback::Kind::HwCompletion;

                let time = if presentation_time.is_zero() {
                    now
                } else {
                    flags.insert(wp_presentation_feedback::Kind::HwClock);
                    presentation_time
                };

                presentation_feedback.presented::<_, Monotonic>(time, refresh, seq, flags);
            }
            Ok(None) => (),
            Err(err) => {
                warn!("Error during rendering: {:?}", err);
                match err {
                    SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
                    _ => (),
                }
            }
        };

        // Now update the frameclock
        output_state.frame_clock.present(presentation_time);

        if redraw_queued || output_state.animations_running {
            fht.queue_redraw(&surface.output);
        } else {
            fht.send_frames(&surface.output);
        }
    }

    pub fn switch_vt(&mut self, vt_num: i32) {
        self.devices.values_mut().for_each(|device| {
            // FIX: Reset overlay planes when changing VTs since some compositors
            // don't use then and as a result don't clean them.
            let _ = device.drm.reset_state();
        });

        if let Err(err) = self.session.change_vt(vt_num) {
            error!(?err, "Failed to switch virtual terminals")
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
    output_global: GlobalId,
    compositor: GbmDrmCompositor,
    dmabuf_feedback: Option<SurfaceDmabufFeedback>,
}

pub type GbmDrmCompositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,
    GbmDevice<DrmDeviceFd>,
    OutputPresentationFeedback,
    DrmDeviceFd,
>;

fn get_surface_dmabuf_feedback(
    primary_gpu: DrmNode,
    render_node: DrmNode,
    gpu_manager: &mut GpuManager<GbmGlesBackend<GlowRenderer, DrmDeviceFd>>,
    compositor: &GbmDrmCompositor,
) -> Option<SurfaceDmabufFeedback> {
    let primary_formats = gpu_manager
        .single_renderer(&primary_gpu)
        .ok()?
        .dmabuf_formats();

    let render_formats = gpu_manager
        .single_renderer(&render_node)
        .ok()?
        .dmabuf_formats();

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
    let planes_formats = surface
        .plane_info()
        .formats
        .iter()
        .copied()
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

const DAMAGE_COLOR: Color32F = Color32F::new(0.05, 0.0, 0.0, 0.05);

fn draw_damage<'a>(
    output: &Output,
    dt: &mut Option<OutputDamageTracker>,
    elements: &[FhtRenderElement<UdevRenderer<'a>>],
) -> Vec<FhtRenderElement<UdevRenderer<'a>>> {
    let dt = dt.get_or_insert_with(|| OutputDamageTracker::from_output(&output));
    let Ok((Some(damage), _)) = dt.damage_output(1, elements) else {
        return vec![];
    };

    let mut damage_elements = vec![];
    for damage_rect in damage {
        let damage_element: DebugRenderElement = SolidColorRenderElement::new(
            Id::new(),
            *damage_rect,
            CommitCounter::default(),
            DAMAGE_COLOR,
            Kind::Unspecified,
        )
        .into();
        damage_elements.push(damage_element.into())
    }

    damage_elements
}

fn get_property_val(
    device: &impl drm::control::Device,
    handle: impl ResourceHandle,
    name: &str,
) -> anyhow::Result<(
    drm::control::property::ValueType,
    drm::control::property::RawValue,
)> {
    let props = device.get_properties(handle)?;
    let (prop_handles, values) = props.as_props_and_values();
    for (&prop, &val) in prop_handles.iter().zip(values.iter()) {
        let info = device.get_property(prop)?;
        if Some(name) == info.name().to_str().ok() {
            let val_type = info.value_type();
            return Ok((val_type, val));
        }
    }
    anyhow::bail!("No prop found for {}", name)
}

/// Calculate the refresh rate, in seconds of this [`Mode`](drm::control::Mode).
///
/// Code copied from mutter.
fn calculate_refresh_rate(mode: &drm::control::Mode) -> f64 {
    let htotal = mode.hsync().2 as u64;
    let vtotal = mode.vsync().2 as u64;
    let vscan = mode.vscan() as u64;

    if htotal <= 0 || vtotal <= 0 {
        return 0f64;
    }
    let numerator = mode.clock() as u64 * 1_000_000;
    let denominator = vtotal * htotal * (if vscan > 1 { vscan } else { 1 });

    (numerator / denominator) as f64
}

/// Gets the mode that matches the given description the closest.
///
/// - If there's a match, this returns `Ok((mode, false))`
/// - If nothing matches, this returns `Ok((preferred_mode, true))`
/// - If there's no match/preferred mode, this returns None
fn get_matching_mode(
    modes: &[drm::control::Mode],
    width: u16,
    height: u16,
    refresh: Option<f64>,
) -> Option<(drm::control::Mode, bool)> {
    if modes.is_empty() {
        return None;
    }

    if let Some(refresh) = refresh {
        let refresh_milli_hz = (refresh * 1000.).round() as i32;
        if let Some(mode) = modes
            .iter()
            .find(|mode| {
                mode.size() == (width, height) && refresh_milli_hz == get_refresh_milli_hz(mode)
            })
            .copied()
        {
            return Some((mode, true));
        }
    } else {
        // User just wants highest refresh rate
        let mut matching_modes = modes
            .iter()
            .filter(|mode| mode.size() == (width, height))
            .copied()
            .collect::<Vec<_>>();
        matching_modes.sort_by_key(|mode| mode.vrefresh());

        if let Some(mode) = matching_modes.first() {
            return Some((*mode, true));
        }
    }

    // Last try: find a preferred mode.
    modes
        .iter()
        .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
        .map(|pref| (*pref, true))
}

/// Get a [`Mode`](drm::control::Mode)'s refresh rate in millihertz
fn get_refresh_milli_hz(mode: &drm::control::Mode) -> i32 {
    let clock = mode.clock() as u64;
    let htotal = mode.hsync().2 as u64;
    let vtotal = mode.vsync().2 as u64;

    let mut refresh = (clock * 1_000_000 / htotal + vtotal / 2) / vtotal;

    if mode.flags().contains(ModeFlags::INTERLACE) {
        refresh *= 2;
    }

    if mode.flags().contains(ModeFlags::DBLSCAN) {
        refresh /= 2;
    }

    if mode.vscan() > 1 {
        refresh /= mode.vscan() as u64;
    }

    refresh as i32
}
