use std::collections::HashMap;
use std::time::Duration;

use anyhow::Context as _;
use calloop::timer::{TimeoutAction, Timer};
use calloop::RegistrationToken;
use fht_compositor_config::VrrMode;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::GbmAllocator;
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager};
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmNode, DrmSurface, VrrSupport};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::multigpu::gbm::GbmGlesBackend;
use smithay::backend::renderer::multigpu::GpuManager;
use smithay::backend::renderer::ImportDma as _;
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::output::{Output, PhysicalProperties, Subpixel};
use smithay::reexports::drm::control::{connector, crtc};
use smithay::reexports::drm::Device as _;
use smithay::reexports::rustix::path::Arg as _;
use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags;
use smithay::reexports::{drm, gbm};
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;
use smithay::wayland::drm_lease::{DrmLease, DrmLeaseBuilder, DrmLeaseRequest, DrmLeaseState, LeaseRejected};
use smithay_drm_extras::display_info;
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};

use super::Surface;
use super::{
    generate_output_render_elements, get_property_val, UdevOutputData, UdevRenderer,
    mode::*,
};
use crate::renderer::blur::EffectsFramebuffers;
use crate::renderer::FhtRenderElement;
use crate::state::{Fht, State, SurfaceDmabufFeedback};

// The main output manager used by the compositor.
// There's nothing special around here, mostly cvopied from anvil.
type OutputManager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    OutputPresentationFeedback,
    DrmDeviceFd,
>;
type DeviceOutput = DrmOutput<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    OutputPresentationFeedback,
    DrmDeviceFd,
>;

pub struct Device {
    node: DrmNode,
    // FIXME: Perhaps if everything was internally managed, it would be much nicer?
    // udev is pretty complicated, and trying to encapsulate everything into a private struct would
    // not work really well in terms of DX.
    pub(super) surfaces: HashMap<crtc::Handle, Surface>,
    pub(super) non_desktop_connectors: Vec<(connector::Handle, crtc::Handle)>,
    lease_state: Option<DrmLeaseState>,
    active_leases: Vec<DrmLease>,
    pub(super) drm_output_manager: OutputManager,
    #[allow(unused)] // only read when using xdg-screencast-portal
    gbm: gbm::Device<DrmDeviceFd>,
    pub(super) drm_scanner: DrmScanner,
    render_node: DrmNode,
    drm_registration_token: RegistrationToken,
}

impl Device {
    // NOTE: Most of the device creation part still is inside UdevBackend, since it requires stuff
    // like the GPU manager, session state, etc. There's also special handling for the primary node.
    //
    // In turn this means it's just a basic constructor, and not actually doing anything expect
    // populating fields.
    pub fn new(
        node: DrmNode,
        lease_state: Option<DrmLeaseState>,
        drm_output_manager: OutputManager,
        gbm: gbm::Device<DrmDeviceFd>,
        render_node: DrmNode,
        drm_registration_token: RegistrationToken,
    ) -> Self {
        Self {
            node,
            surfaces: HashMap::new(),
            non_desktop_connectors: Vec::new(),
            lease_state,
            active_leases: Vec::new(),
            drm_output_manager,
            gbm,
            drm_scanner: DrmScanner::new(),
            render_node,
            drm_registration_token,
        }
    }

    pub fn gbm_device(&self) -> gbm::Device<DrmDeviceFd> {
        self.gbm.clone()
    }

    /// Resets this drm device.
    /// This in turns will try to reset all the [`DrmOutput`]s handled by this device.
    pub fn reset(&mut self) {
        self.drm_output_manager
            .device_mut()
            .reset_state()
            .expect("failed to reset drm device");
    }

    /// Pauses/disables this device.
    ///
    /// This will cause the session to stop sending/reading events from the DRM device fd, hence
    /// "freezing" the session completely.
    pub fn pause(&mut self) {
        self.drm_output_manager.pause();
        self.active_leases.clear();
        if let Some(leasing_state) = self.lease_state.as_mut() {
            leasing_state.suspend();
        }
    }

    /// Re-activate the device.
    // To my understanding, you can call this even before doing `self.pause()`, as a way to force
    // reset this device and all its surfaces. However, I don't know about how good of an idea it
    // may be.
    pub fn activate(&mut self) {
        // if we do not care about flicking (caused by modesetting) we could just
        // pass true for disable connectors here. this would make sure our drm
        // device is in a known state (all connectors and planes disabled).
        // but for demonstration we choose a more optimistic path by leaving the
        // state as is and assume it will just work. If this assumption fails
        // we will try to reset the state when trying to queue a frame.
        if let Err(err) = self.drm_output_manager.lock().activate(false) {
            error!(?err, "Failed to activate DRM output manager");
        }
        if let Some(leasing_state) = self.lease_state.as_mut() {
            leasing_state.resume::<State>();
        }
        if let Err(err) = self.drm_output_manager.device_mut().reset_state() {
            warn!(?err, "Failed to reset drm surface state");
        }
    }

    /// Try to update this device.
    ///
    /// This essentially scans for new connectors and adds them to [`Device::surfaces`] or
    /// [`Device::non_desktop_connectors`].
    pub fn update(
        &mut self,
        fht: &mut Fht,
        primary_render_node: DrmNode,
        gpu_manager: &mut GpuManager<GbmGlesBackend<GlowRenderer, DrmDeviceFd>>,
    ) -> anyhow::Result<()> {
        let Ok(result) = self
            .drm_scanner
            .scan_connectors(self.drm_output_manager.device())
            .inspect_err(|err| warn!(?err, ?self.node, "Failed to scan connectors for device"))
        else {
            return Ok(());
        };

        for event in result {
            match event {
                DrmScanEvent::Connected { connector, crtc } => {
                    if let Some(crtc) = crtc {
                        if let Err(err) = self.add_connector(
                            crtc,
                            connector,
                            primary_render_node,
                            gpu_manager,
                            fht,
                        ) {
                            error!(?crtc, ?err, "Failed to add connector to device")
                        };
                    }
                    // No crtc, can't do much for you since I dont even know WHAT you connected.
                }
                DrmScanEvent::Disconnected { connector, crtc } => {
                    if let Some(crtc) = crtc {
                        if let Err(err) =
                            self.remove_connector(crtc, connector.handle(), gpu_manager, fht)
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

    pub fn add_connector(
        &mut self,
        crtc: crtc::Handle,
        conn: connector::Info,
        primary_render_node: DrmNode,
        gpu_manager: &mut GpuManager<GbmGlesBackend<GlowRenderer, DrmDeviceFd>>,
        fht: &mut Fht,
    ) -> anyhow::Result<()> {
        debug!(?self.node, ?crtc, "Connector connected");

        let mut renderer = gpu_manager
            .single_renderer(&self.render_node)
            .context("Missing device renderer")?;

        let output_name = format!("{}-{}", conn.interface().as_str(), conn.interface_id());
        debug!(?crtc, ?output_name, "Trying to setup connector");
        let drm_device = self.drm_output_manager.device();

        // Fetch information using libdisplay-info
        let info = display_info::for_connector(drm_device, conn.handle());
        let make = info
            .as_ref()
            .and_then(|info| info.make())
            .unwrap_or_else(|| "Unknown".into());
        let model = info
            .as_ref()
            .and_then(|info| info.model())
            .unwrap_or_else(|| "Unknown".into());
        let serial_number = info
            .as_ref()
            .and_then(|info| info.serial())
            .unwrap_or_else(|| "Unknown".into());

        if !is_desktop_connector(drm_device, conn.handle()) {
            if let Some(lease_state) = self.lease_state.as_mut() {
                debug!(?conn, "Setting up connector for leasing");
                self.non_desktop_connectors.push((conn.handle(), crtc));

                lease_state.add_connector::<State>(
                    conn.handle(),
                    output_name.clone(),
                    format!("{make}-{model}"),
                );
            }
        }

        // Fetch the request mode and output from the configuration.
        //
        // FIXME: in the future the configuration should try to match with libdisplay-info
        // information. This would make it much more stable than connector names. We can still have
        // a fallback on them if needed, +a lot of WL tools expect these anyway.
        let mut new_scale = None;
        let mut new_transform = None;
        // Sometimes DRM connectors can have custom modes.
        // ---
        // The user specifies one, for example 1920x1080@165 and we build a new DrmMode out
        // of this and the connector info. We test it, it works, nice, otherwise, use
        // closest requested, or fallback.
        let modes = conn.modes();
        let default_mode = get_default_mode(modes);
        let mut requested_mode = None;
        let output_config = fht
            .config
            .outputs
            .get(&output_name)
            .cloned()
            .unwrap_or_default();

        if let Some((width, height, refresh)) = output_config.mode {
            // If we can find a pre-defined mode from the output with the given parameters,
            // everything is fine!
            requested_mode = get_matching_mode(modes, width, height, refresh)
                // Otherwise try to generate a mode with CVT calculations,
                // though this doesn't always work.
                .or_else(|| get_custom_mode(width, height, refresh));
        }

        if let Some(transform) = output_config.transform {
            new_transform = Some(transform.into());
        }

        if let Some(scale) = output_config.scale {
            new_scale = Some(smithay::output::Scale::Integer(scale));
        }

        // Create the output object and expose it's wl_output global to clients
        let physical_size = conn
            .size()
            .map(|(w, h)| (w as i32, h as i32))
            .unwrap_or((0, 0))
            .into();
        let physical_properties = PhysicalProperties {
            size: physical_size,
            subpixel: match conn.subpixel() {
                connector::SubPixel::HorizontalRgb => Subpixel::HorizontalRgb,
                connector::SubPixel::HorizontalBgr => Subpixel::HorizontalBgr,
                connector::SubPixel::VerticalRgb => Subpixel::VerticalRgb,
                connector::SubPixel::VerticalBgr => Subpixel::VerticalBgr,
                connector::SubPixel::None => Subpixel::None,
                _ => Subpixel::Unknown,
            },
            make,
            model,
            serial_number,
        };

        // Now create the wl_output object to expose it to clients.
        // The global will be created with Fht::add_output
        let output = Output::new(output_name.clone(), physical_properties);
        for mode in modes {
            let wl_mode = smithay::output::Mode::from(*mode);
            output.add_mode(wl_mode);
        }

        let mut refresh_interval =
            Duration::from_secs_f64(1_000f64 / calculate_refresh_rate(&default_mode));
        let new_mode = requested_mode
            .map(|mode| {
                refresh_interval =
                    Duration::from_secs_f64(1_000f64 / calculate_refresh_rate(&mode));
                mode.into()
            })
            .unwrap_or_else(|| default_mode.into());
        output.set_preferred(new_mode); // adds the mode if its a custom CVT one
        output.change_current_state(Some(new_mode), new_transform, new_scale, None);
        output
            .user_data()
            // This ID is used to match and output and a udev surface
            .insert_if_missing(|| UdevOutputData {
                device: self.node,
                crtc,
            });
        // After setting up all the data, expose the output to the wayland clients.
        let output_global = output.create_global::<State>(&fht.display_handle);

        let mut planes = drm_device.planes(&crtc).context("Failed to get planes")?;
        if is_nvidia(drm_device) {
            // NVIDIA doesn't support overlay planes.
            planes.overlay.clear();
        }

        // Since we only use 8-bit formats, we fix the "max bpc" property
        if let Err(err) = set_max_bpc(self.drm_output_manager.device(), conn.handle(), 8) {
            warn!(?err, "Failed to set max bpc for output {output_name}");
        }

        // When initializing the DRM output, we use the default mode to initialize, since sometimes
        // using a custom mode right now might cause the DrmOutput to fail initializing.
        //
        // DrmCompositor will automatically try to switch to the active output mode after being
        // initialized.
        let render_elements = generate_output_render_elements(fht, &mut renderer);
        let mut drm_output = match self
            .drm_output_manager
            .lock()
            .initialize_output::<_, FhtRenderElement<UdevRenderer<'_>>>(
                crtc,
                default_mode,
                &[conn.handle()],
                &output,
                Some(planes.clone()),
                &mut renderer,
                &render_elements,
            ) {
            Ok(output) => output,
            Err(err) => {
                anyhow::bail!("Failed to create DRM output: {err:?}");
            }
        };

        let mut vrr_enabled = None;
        if vrr_suported(&drm_output, conn.handle()) {
            // Only enable if its required for NOW. We can change the VRR state down the line when
            // a window gets exposed onto the primary plane.
            let enable = output_config.vrr == VrrMode::On;
            drm_output.with_compositor(|c| {
                if let Err(err) = c.use_vrr(enable) {
                    warn!(
                        ?err,
                        ?output_name,
                        vrr = enable,
                        "Failed to update VRR state on new output"
                    );
                }

                vrr_enabled = Some(match c.vrr_enabled() {
                    true => VrrMode::On,
                    false => VrrMode::Off,
                });
            })
        } else if matches!(output_config.vrr, VrrMode::On | VrrMode::OnDemand) {
            warn!(
                ?output_name,
                "Cannot enable VRR on output since its not supported!"
            );
        }

        // Apply the requested mode if any.
        //
        // NOTE: This has to be done after output creation. For some reason, trying to use high
        // pixel clock modes when initializing the DRM output doesn't work for some reason, so we.
        if let Some(mode) = requested_mode {
            if let Err(err) = drm_output.use_mode(mode, &mut renderer, &render_elements) {
                error!(?err, "Failed to apply custom mode for {output_name}");
            }
        }

        fht.add_output(output.clone(), Some(refresh_interval), vrr_enabled);

        // NOTE: In contrary to Shaders, the effects frame buffers are kept on a per-output basis
        // to avoid noise and pollution from other outputs leaking into eachother
        EffectsFramebuffers::init_for_output(&output, &mut renderer);

        let dmabuf_feedback = drm_output.with_compositor(|compositor| {
            // We only render on one primary gpu, so we don't have to manage different feedbacks
            // based on render nodes.
            get_surface_dmabuf_feedback(
                primary_render_node,
                self.render_node,
                gpu_manager,
                compositor.surface(),
            )
        });

        fht.queue_redraw(&output);
        let surface = Surface {
            render_node: self.render_node.clone(),
            connector: conn.handle(),
            output,
            output_global,
            drm_output,
            dmabuf_feedback,
            gamma_blob: None,
        };
        self.surfaces.insert(crtc, surface);

        Ok(())
    }

    pub fn remove_connector(
        &mut self,
        crtc: crtc::Handle,
        connector: connector::Handle,
        gpu_manager: &mut GpuManager<GbmGlesBackend<GlowRenderer, DrmDeviceFd>>,
        fht: &mut Fht,
    ) -> anyhow::Result<()> {
        debug!(?self.node, ?crtc, "Connector disconnected");

        if let Some(pos) = self
            .non_desktop_connectors
            .iter()
            .position(|(handle, _)| *handle == connector)
        {
            // Connector is non-desktop, just disable leasing and unregister it.
            let _ = self.non_desktop_connectors.remove(pos);
            if let Some(leasing_state) = self.lease_state.as_mut() {
                leasing_state.withdraw_connector(connector);
            }
            return Ok(());
        }

        let Some(surface) = self.surfaces.remove(&crtc) else {
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

        let mut renderer = gpu_manager.single_renderer(&self.render_node).unwrap();
        let render_elements = generate_output_render_elements(fht, &mut renderer);
        // Read the comment on try_to_restore_modifiers for why we should call this now.
        _ = self
            .drm_output_manager
            .lock()
            .try_to_restore_modifiers::<_, FhtRenderElement<UdevRenderer<'_>>>(
                &mut renderer,
                &render_elements,
            );

        Ok(())
    }

    /// Remove this device.
    pub fn remove(
        mut self,
        fht: &mut Fht,
        gpu_manager: &mut GpuManager<GbmGlesBackend<GlowRenderer, DrmDeviceFd>>,
    ) -> anyhow::Result<()> {
        let crtcs = self
            .drm_scanner
            .crtcs()
            .map(|(info, crtc)| (info.clone(), crtc))
            .collect::<Vec<_>>();
        for (conn, crtc) in crtcs {
            // At this point this device won't be used again, so there's no point in caring about
            // its connectors cleaning up properly. This does affect hotpluggin, I think?
            _ = self.remove_connector(crtc, conn.handle(), gpu_manager, fht);
        }

        if let Some(mut leasing_state) = self.lease_state.take() {
            leasing_state.disable_global::<State>();
        }

        gpu_manager.as_mut().remove_node(&self.render_node);
        fht.loop_handle.remove(self.drm_registration_token);

        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.drm_output_manager.device().is_active()
    }

    pub fn lease_state(&mut self) -> Option<&mut DrmLeaseState> {
        self.lease_state.as_mut()
    }

    /// Handle a lease request.
    ///
    /// There's no special handling for now, it just always gives out leases.
    pub fn lease_request(
        &self,
        request: DrmLeaseRequest,
    ) -> Result<DrmLeaseBuilder, LeaseRejected> {
        let drm_device = self.drm_output_manager.device();
        let mut builder = DrmLeaseBuilder::new(drm_device);
        for conn in request.connectors {
            if let Some((_, crtc)) = self
                .non_desktop_connectors
                .iter()
                .find(|(handle, _)| *handle == conn)
            {
                builder.add_connector(conn);
                builder.add_crtc(*crtc);
                let planes = drm_device.planes(crtc).map_err(LeaseRejected::with_cause)?;

                let (primary_plane, primary_plane_claim) = planes
                    .primary
                    .iter()
                    .find_map(|plane| {
                        drm_device
                            .claim_plane(plane.handle, *crtc)
                            .map(|claim| (plane, claim))
                    })
                    .ok_or_else(LeaseRejected::default)?;
                builder.add_plane(primary_plane.handle, primary_plane_claim);
                if let Some((cursor, claim)) = planes.cursor.iter().find_map(|plane| {
                    drm_device
                        .claim_plane(plane.handle, *crtc)
                        .map(|claim| (plane, claim))
                }) {
                    builder.add_plane(cursor.handle, claim);
                }
            } else {
                warn!(
                    ?conn,
                    "Lease requested for desktop connector, denying request"
                );
                return Err(LeaseRejected::default());
            }
        }

        Ok(builder)
    }

    pub fn add_active_lease(&mut self, lease: DrmLease) {
        self.active_leases.push(lease);
    }

    pub fn remove_lease(&mut self, lease_id: u32) {
        self.active_leases.retain(|l| l.id() != lease_id);
    }
}

fn is_desktop_connector(device: &DrmDevice, handle: connector::Handle) -> bool {
    match get_property_val(device, handle, "non-desktop") {
        Ok((ty, val)) => ty.convert_value(val).as_boolean().unwrap_or(false),
        Err(_) => {
            warn!(?handle, "Failed to determine if connector is non-desktop");
            false
        }
    }
}

fn is_nvidia(device: &DrmDevice) -> bool {
    let Ok(driver) = device.get_driver() else {
        warn!(?device, "Failed to determine if device is NVIDIA");
        return false;
    };

    let mut is_nvidia = false;
    is_nvidia |= driver
        .name
        .to_string_lossy()
        .to_lowercase()
        .contains("nvidia");
    is_nvidia |= driver
        .desc
        .to_string_lossy()
        .to_lowercase()
        .contains("nvidia");

    is_nvidia
}

/// Sets the maximum value of bits per color for a given connector
// https://lists.freedesktop.org/archives/dri-devel/2018-September/190283.html
fn set_max_bpc(
    device: &impl drm::control::Device,
    connector: connector::Handle,
    max_bpc: u64,
) -> anyhow::Result<()> {
    let props = device
        .get_properties(connector)
        .context("failed to get connector props")?;
    for (prop, value) in props {
        let info = device
            .get_property(prop)
            .context("failed to get property")?;
        if info.name().as_str() != Ok("max bpc") {
            continue; // no what we are searching for
        }

        let drm::control::property::ValueType::UnsignedRange(min, max) = info.value_type() else {
            anyhow::bail!("wrong value type")
        };

        let bpc = max_bpc.clamp(min, max);

        let drm::control::property::Value::UnsignedRange(value) =
            info.value_type().convert_value(value)
        else {
            anyhow::bail!("wrong property type")
        };
        if value == bpc {
            return Ok(()); // no changes
        }

        device
            .set_property(
                connector,
                prop,
                drm::control::property::Value::UnsignedRange(bpc).into(),
            )
            .context("error setting property")?;
    }

    Ok(())
}

fn vrr_suported(output: &DeviceOutput, conn: connector::Handle) -> bool {
    output.with_compositor(|compositor| match compositor.vrr_supported(conn) {
        Ok(VrrSupport::NotSupported) => false,
        Ok(VrrSupport::RequiresModeset | VrrSupport::Supported) => true,
        Err(_) => {
            warn!(?conn, "Failed to query VRR support");
            false
        }
    })
}

fn get_surface_dmabuf_feedback(
    primary_gpu: DrmNode,
    render_node: DrmNode,
    gpus: &mut GpuManager<GbmGlesBackend<GlowRenderer, DrmDeviceFd>>,
    surface: &DrmSurface,
) -> Option<SurfaceDmabufFeedback> {
    let primary_formats = gpus.single_renderer(&primary_gpu).ok()?.dmabuf_formats();
    let render_formats = gpus.single_renderer(&render_node).ok()?.dmabuf_formats();

    let all_render_formats = primary_formats
        .iter()
        .chain(render_formats.iter())
        .copied()
        .collect::<FormatSet>();

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
        .collect::<FormatSet>()
        .intersection(&all_render_formats)
        .copied()
        .collect::<FormatSet>();

    let builder = DmabufFeedbackBuilder::new(primary_gpu.dev_id(), primary_formats);
    let render_feedback = builder
        .clone()
        .add_preference_tranche(render_node.dev_id(), None, render_formats.clone())
        .build()
        .unwrap();

    let scanout_feedback = builder
        .add_preference_tranche(
            surface.device_fd().dev_id().unwrap(),
            Some(TrancheFlags::Scanout),
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
