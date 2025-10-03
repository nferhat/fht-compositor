use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use calloop::futures::Scheduler;
use fht_compositor_config::{BlurOverrides, BorderOverrides, DecorationMode, ShadowOverrides};
use smithay::backend::renderer::element::utils::select_dmabuf_feedback;
use smithay::backend::renderer::element::{
    default_primary_scanout_output_compare, PrimaryScanoutOutput, RenderElementStates,
};
use smithay::desktop::utils::{
    send_dmabuf_feedback_surface_tree, send_frames_surface_tree,
    surface_presentation_feedback_flags_from_states, surface_primary_scanout_output,
    take_presentation_feedback_surface_tree, under_from_surface_tree,
    update_surface_primary_scanout_output, OutputPresentationFeedback,
};
use smithay::desktop::{layer_map_for_output, LayerSurface, PopupManager, WindowSurfaceType};
use smithay::input::keyboard::{KeyboardHandle, Keysym, XkbConfig};
use smithay::input::pointer::{CursorImageStatus, MotionEvent, PointerHandle};
use smithay::input::{Seat, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::{self, LoopHandle, LoopSignal, RegistrationToken};
use smithay::reexports::input::{self, DeviceCapability, SendEventsMode};
use smithay::reexports::wayland_server::backend::ClientData;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::{Clock, IsAlive, Logical, Monotonic, Point, Rectangle, SERIAL_COUNTER};
use smithay::wayland::alpha_modifier::AlphaModifierState;
use smithay::wayland::compositor::{
    with_states, with_surface_tree_downward, CompositorClientState, CompositorState, SurfaceData,
    TraversalAction,
};
use smithay::wayland::content_type::ContentTypeState;
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::dmabuf::{DmabufFeedback, DmabufState};
use smithay::wayland::foreign_toplevel_list::ForeignToplevelListState;
use smithay::wayland::fractional_scale::{with_fractional_scale, FractionalScaleManagerState};
use smithay::wayland::idle_inhibit::IdleInhibitManagerState;
use smithay::wayland::idle_notify::IdleNotifierState;
use smithay::wayland::input_method::InputMethodManagerState;
use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::pointer_constraints::PointerConstraintsState;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::security_context::{SecurityContext, SecurityContextState};
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::session_lock::SessionLockManagerState;
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer, WlrLayerShellState};
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::dialog::XdgDialogState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use smithay::wayland::tablet_manager::TabletManagerState;
use smithay::wayland::text_input::TextInputManagerState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;
use smithay::wayland::xdg_activation::XdgActivationState;
use smithay::wayland::xdg_foreign::XdgForeignState;

use crate::backend::Backend;
use crate::config::ui as config_ui;
use crate::cursor::CursorThemeManager;
use crate::focus_target::{KeyboardFocusTarget, PointerFocusTarget};
use crate::frame_clock::FrameClock;
use crate::handlers::session_lock::LockState;
use crate::layer::MappedLayer;
use crate::output::{self, OutputExt, RedrawState};
#[cfg(feature = "xdg-screencast-portal")]
use crate::portals::screencast::{
    self, CursorMode, ScreencastSession, ScreencastSource, StreamMetadata,
};
use crate::protocols::output_management::OutputManagementManagerState;
use crate::protocols::screencopy::ScreencopyManagerState;
use crate::renderer::blur::EffectsFramebuffers;
use crate::space::{self, Space, WorkspaceId};
#[cfg(feature = "xdg-screencast-portal")]
use crate::utils::pipewire::{CastId, CastSource, PipeWire, PwToCompositor};
use crate::utils::{get_monotonic_time, RectCenterExt};
use crate::window::Window;
use crate::{cli, ipc};

pub struct State {
    pub fht: Fht,
    pub backend: Backend,
}

impl State {
    pub fn new(
        dh: &DisplayHandle,
        loop_handle: LoopHandle<'static, State>,
        loop_signal: LoopSignal,
        config_path: Option<std::path::PathBuf>,
        ipc_server: Option<ipc::Server>,
        backend: Option<crate::cli::BackendType>,
        _socket_name: String,
    ) -> Self {
        #[allow(unused)]
        let mut fht = Fht::new(dh, loop_handle, loop_signal, ipc_server, config_path);
        #[allow(unused)]
        let backend: crate::backend::Backend = if let Some(backend_type) = backend {
            match backend_type {
                #[cfg(feature = "winit-backend")]
                cli::BackendType::Winit => crate::backend::winit::WinitData::new(&mut fht)
                    .unwrap()
                    .into(),
                #[cfg(feature = "udev-backend")]
                cli::BackendType::Udev => crate::backend::udev::UdevData::new(&mut fht)
                    .unwrap()
                    .into(),
                #[cfg(feature = "headless-backend")]
                cli::BackendType::Headless => {
                    crate::backend::headless::HeadlessData::new(&mut fht).into()
                }
            }
        } else if std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok() {
            info!("Detected (WAYLAND_)DISPLAY. Running in nested Winit window");
            #[cfg(feature = "winit-backend")]
            {
                crate::backend::winit::WinitData::new(&mut fht)
                    .unwrap()
                    .into()
            }
            #[cfg(not(feature = "winit-backend"))]
            panic!("Winit backend not enabled on this build! Enable the 'winit-backend' feature when building")
        } else {
            info!("Running from TTY, initializing Udev backend");
            #[cfg(feature = "udev-backend")]
            {
                crate::backend::udev::UdevData::new(&mut fht)
                    .unwrap()
                    .into()
            }
            #[cfg(not(feature = "udev-backend"))]
            panic!("Udev backend not enabled on this build! Enable the 'udev-backend' feature when building")
        };

        #[allow(unreachable_code)]
        Self { fht, backend }
    }

    pub fn dispatch(&mut self) -> anyhow::Result<()> {
        crate::profile_function!();
        self.fht.space.refresh();
        self.fht.popups.cleanup();
        self.fht.refresh_idle_inhibit();

        // This must be called before redrawing the outputs since the render elements depend on
        // up-to-date focus and window rules, so update them before redrawing the outputs
        self.update_keyboard_focus();
        self.fht.resolve_rules_for_all_windows_if_needed();

        {
            crate::profile_scope!("refresh_and_redraw_outputs");
            let mut outputs_to_redraw = vec![];
            let locked = self.fht.is_locked();
            for output in self.fht.space.outputs() {
                let output_state = self.fht.output_state.get_mut(output).unwrap();
                if !locked {
                    // Take away the lock surface
                    output_state.lock_backdrop = None;
                    output_state.lock_surface = None;
                } else {
                    let _ = output_state
                        .lock_surface
                        .take_if(|surface| !surface.alive());
                }

                if output_state.redraw_state.is_queued() {
                    outputs_to_redraw.push(output.clone());
                }
            }

            for output in outputs_to_redraw {
                self.redraw(output);
            }
        };
        self.fht.lock_state = match std::mem::take(&mut self.fht.lock_state) {
            // Switch from pending to locked when we finished drawing a backdrop at least once.
            LockState::Pending(locker)
                if self.fht.space.outputs().all(|output| {
                    self.fht
                        .output_state
                        .get(output)
                        .unwrap()
                        .lock_backdrop
                        .is_some()
                }) =>
            {
                locker.lock();
                LockState::Locked
            }
            state => state,
        };

        // NOTE: If we cleared lock surface, `SessionLockHandler::unlock` will call
        // `State::update_keyboard_focus` to appropriatly update the focus of the keyboard.
        // This is done only to keep pointer contents updated.
        self.update_pointer_focus();

        {
            crate::profile_scope!("flush_clients");
            self.fht
                .display_handle
                .flush_clients()
                .context("Failed to flush_clients!")?;
        }

        // We do this after everything to make sure we send accurate state.
        self.fht.refresh_ipc();

        Ok(())
    }

    /// Refresh the pointer focus.
    pub fn update_pointer_focus(&mut self) {
        crate::profile_scope!("refresh_pointer_focus");
        // We try to update the pointer focus. If the new one is not the same as the previous one we
        // encountered, we send a motion and frame event to the new one.
        let pointer = self.fht.pointer.clone();
        let pointer_loc = pointer.current_location();
        let new_focus = self.fht.focus_target_under(pointer_loc);
        if new_focus.as_ref().map(|(ft, _)| ft) == pointer.current_focus().as_ref() {
            return; // No updates, keep going
        }

        pointer.motion(
            self,
            new_focus,
            &MotionEvent {
                location: pointer_loc,
                serial: SERIAL_COUNTER.next_serial(),
                time: get_monotonic_time().as_millis() as u32,
            },
        );
        // After motion, try to activate new pointer constraint under surface
        self.fht.activate_pointer_constraint();

        pointer.frame(self);
    }

    pub fn new_client_state(&self) -> ClientState {
        ClientState {
            compositor: CompositorClientState::default(),
            security_context: None,
        }
    }

    pub fn redraw(&mut self, output: Output) {
        crate::profile_function!();

        // Verify our invariant.
        let output_state = self.fht.output_state.get_mut(&output).unwrap();
        assert!(output_state.redraw_state.is_queued());

        // Advance animations.
        let target_presentation_time = output_state.frame_clock.next_presentation_time();
        let animations_running = {
            crate::profile_scope!("advance_animations");
            let mut ongoing = self.fht.config_ui.advance_animations(
                target_presentation_time,
                !self.fht.config.animations.disable,
            );
            // If we finished animating the config_ui this means its hidden.
            // Clear the output its opened on
            let _ = self.fht.config_ui_output.take_if(|_| !ongoing);

            ongoing |= self
                .fht
                .space
                .advance_animations(target_presentation_time, &output);

            ongoing
        };

        let output_state = self.fht.output_state.get_mut(&output).unwrap();
        output_state.animations_running = animations_running;

        // Then ask the backend to render.
        // if res.is_err() == something wrong happened and we didnt render anything.
        // if res == Ok(true) we rendered and submitted a new buffer
        // if res == Ok(false) we rendered but had no damage to submit
        let res = self
            .backend
            .render(&mut self.fht, &output, target_presentation_time);

        {
            let output_state = self.fht.output_state.get_mut(&output).unwrap();
            if res.is_err() {
                // Update the redraw state on failed render.
                output_state.redraw_state =
                    if let RedrawState::WaitingForEstimatedVblankTimer { token, .. } =
                        output_state.redraw_state
                    {
                        RedrawState::WaitingForEstimatedVblankTimer {
                            token,
                            queued: false,
                        }
                    } else {
                        RedrawState::Idle
                    };
            }
        }

        // Update vrr state after rendering.
        //
        // By now, all the surfaces on the output will have their primary scanout output decided,
        // and the planes should have been assigned and scanned out by now. We can proceed to update
        // VRR state now.
        self.fht.output_update_vrr(&output);

        // Send frame callbacks
        self.fht.send_frames(&output);
    }

    pub fn reload_config(&mut self) {
        crate::profile_function!();

        let (new_config, paths) =
            match fht_compositor_config::load(self.fht.cli_config_path.clone()) {
                Ok((config, paths)) => {
                    self.fht.config_ui.show(
                        config_ui::Content::Reloaded {
                            paths: paths.clone(),
                        },
                        !self.fht.config.animations.disable,
                    );

                    (config, paths)
                }
                Err(err) => {
                    error!(?err, "Failed to load configuration, using default");
                    self.fht.config_ui.show(
                        config_ui::Content::ReloadError { error: err },
                        !self.fht.config.animations.disable,
                    );
                    // Keep the user with the current configuration
                    return;
                }
            };

        let config_watcher = crate::config::init_watcher(paths, &self.fht.loop_handle)
            .inspect_err(|err| warn!(?err, "Failed to start config file watcher"))
            .ok();
        if let Some(watcher) = std::mem::replace(&mut self.fht.config_watcher, config_watcher) {
            watcher.stop(&self.fht.loop_handle);
            // The associated thread will die alone since it will error out (tx.send will fail since
            // the channel does not exist anymore) So there's nothing todo with the
            // join_handle!
        }
        let old_config = Arc::clone(&self.fht.config);
        let config = Arc::new(new_config);

        // Some invariants must be upheld when reloading the configuration
        // If any reloading function errors out, the configuration is not valid

        let keyboard = self.fht.keyboard.clone();
        if let Err(err) = keyboard.set_xkb_config(self, config.input.keyboard.xkb_config()) {
            error!(?err, "Failed to apply configuration");
            return;
        }

        self.fht.space.reload_config(&config);

        self.fht
            .cursor_theme_manager
            .reload_config(config.cursor.clone());

        // If we made it up to here, the configuration must be valid
        self.fht.config = config;

        if old_config.outputs != self.fht.config.outputs || self.fht.has_transient_output_changes {
            self.fht.reload_output_config();
        }

        // These devices are just handles, so cleaning the devices vector and adding them all
        // back should not be an issue. (input device configuration code in inside
        // add_libinput_device function)
        let devices: Vec<_> = self.fht.devices.drain(..).collect();
        for device in devices {
            self.fht.add_libinput_device(device);
        }

        // For layer shell rules, we only recompute them on layer-shell commit. Some layer shells
        // just don't commit (for example your wallpaper). so we must refresh them at least once
        // here.
        self.fht.resolve_rules_for_all_layer_shells();

        // Queue a redraw to ensure everything is up-to-date visually.
        self.fht
            .space
            .outputs()
            .for_each(|o| EffectsFramebuffers::get(o).optimized_blur_dirty = true);
        self.fht.queue_redraw_all();
    }

    #[cfg(feature = "xdg-screencast-portal")]
    pub fn handle_screencast_request(&mut self, req: screencast::Request) {
        match req {
            screencast::Request::StartCast {
                session_handle,
                metadata_sender,
                source,
                cursor_mode,
            } => {
                if let Err(err) = self.start_cast(
                    session_handle.clone(),
                    metadata_sender.clone(),
                    source,
                    cursor_mode,
                ) {
                    error!(
                        session_handle = session_handle.to_string(),
                        ?err,
                        "Failed to start pipewire screencast"
                    );
                    // If we errored out here we didn't send anything back to the portal yet.
                    // Sending None signifies that we got an error, and to drop the session.
                    let _ = metadata_sender.send_blocking(None);
                }
            }
            screencast::Request::StopCast { cast_id } => {
                self.fht.stop_cast(cast_id);
            }
        }
    }

    #[cfg(feature = "xdg-screencast-portal")]
    fn start_cast(
        &mut self,
        session_handle: zvariant::OwnedObjectPath,
        metadata_sender: async_channel::Sender<Option<StreamMetadata>>,
        mut source: ScreencastSource,
        cursor_mode: CursorMode,
    ) -> anyhow::Result<()> {
        crate::profile_function!();
        // We don't support screencasting on X11 since eh, you prob dont need it.

        use smithay::reexports::calloop;

        #[cfg(not(feature = "udev-backend"))]
        {
            anyhow::bail!("ScreenCast is only supported on udev backend");
        }
        #[cfg(feature = "udev-backend")]
        {
            #[allow(irrefutable_let_patterns)]
            let Backend::Udev(ref mut data) = &mut self.backend
            else {
                anyhow::bail!("screencast is only supported on udev")
            };

            let Some(gbm_device) = data.primary_gbm_device() else {
                anyhow::bail!("no primary GBM device")
            };

            let (cast_source, size, refresh, alpha) = match &mut source {
                ScreencastSource::Output { name } => {
                    let Some(output) = self.fht.output_named(name.as_str()) else {
                        anyhow::bail!("invalid output from screencast source");
                    };

                    let mode = output.current_mode().unwrap();
                    let transform = output.current_transform();
                    let size = transform.transform_size(mode.size);
                    let refresh = mode.refresh as u32;
                    (CastSource::Output(output.downgrade()), size, refresh, false)
                }
                ScreencastSource::Workspace {
                    output: output_name,
                    idx,
                } => {
                    let idx = *idx;
                    let Some(output) = self.fht.output_named(output_name.as_str()) else {
                        anyhow::bail!("invalid output from screencast source");
                    };
                    if idx > 8 {
                        anyhow::bail!("invalid workspace index from screencast source");
                    }

                    let mode = output.current_mode().unwrap();
                    let transform = output.current_transform();
                    let size = transform.transform_size(mode.size);
                    let refresh = mode.refresh as u32;
                    (
                        CastSource::Workspace {
                            output: output.downgrade(),
                            index: idx,
                        },
                        size,
                        refresh,
                        false,
                    )
                }
                ScreencastSource::Window { id } => {
                    let mut cast_window = None;
                    for window in self.fht.space.windows() {
                        if window.id() == *id {
                            cast_window = Some(window.clone());
                            break;
                        }
                    }

                    let Some(window) = cast_window else {
                        anyhow::bail!("invalid window from screencast source");
                    };

                    // SAFETY: If a window has a foreign toplevel handle, it is mapped. We remove it
                    // on unmap <=> The window has a WlSurface
                    let output = self
                        .fht
                        .space
                        .output_for_surface(&window.wl_surface().unwrap())
                        .unwrap();
                    let mode = output.current_mode().unwrap();
                    let scale = output.current_scale().integer_scale() as f64;
                    let size = window
                        .bbox_with_popups()
                        .to_physical_precise_round(scale)
                        .size;
                    let refresh = mode.refresh as u32;

                    (CastSource::Window(window.downgrade()), size, refresh, true)
                }
            };

            self.fht.pipewire_initialised.call_once(|| {
                self.fht.pipewire = PipeWire::new(&self.fht.loop_handle)
                    .map_err(|err| warn!(?err, "Failed to initialize PipeWire!"))
                    .ok();
            });

            let Some(pipewire) = self.fht.pipewire.as_mut() else {
                anyhow::bail!("no pipewire")
            };

            let render_formats = self
                .backend
                .with_renderer(|renderer| renderer.egl_context().dmabuf_render_formats().clone())
                .expect("we should be in Udev backend when starting screencast");

            let (to_compositor, from_pw) = calloop::channel::channel();
            let token = self
                .fht
                .loop_handle
                .insert_source(from_pw, |event, (), state| {
                    let calloop::channel::Event::Msg(msg) = event else {
                        return;
                    };
                    match msg {
                        PwToCompositor::Redraw { id, source } => match source {
                            CastSource::Output(weak) => {
                                if let Some(output) = weak.upgrade() {
                                    state.fht.queue_redraw(&output);
                                } else {
                                    warn!(?id, "Received a redraw request for a non-existing output, stopping cast");
                                    state.fht.stop_cast(id);
                                }
                            }

                            // NOTE: For window and workspace screencasts, we don't redraw the output
                            // since they may not forcibly be visible.
                            CastSource::Window(window) => {
                                if window.upgrade().is_none() {
                                    warn!(?id, "Received a redraw request for a closed window, stopping cast");
                                    state.fht.stop_cast(id);
                                }
                            }
                            CastSource::Workspace { output, .. } => {
                                if output.upgrade().is_none() {
                                    warn!(?id, "Received a redraw request for a closed window, stopping cast");
                                    state.fht.stop_cast(id);
                                }
                            }
                        },
                        PwToCompositor::StopCast { id } => {
                            state.fht.stop_cast(id);
                        }
                    }
                })
                .map_err(|err| {
                    anyhow::anyhow!("Failed to insert pipewire channel source: {err:?}")
                })?;

            pipewire.start_cast(
                session_handle,
                cast_source,
                cursor_mode,
                to_compositor,
                token,
                metadata_sender,
                gbm_device,
                &render_formats,
                alpha,
                size,
                refresh,
            )?;

            Ok(())
        }
    }
}

pub struct Fht {
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, State>,
    pub scheduler: Scheduler<()>,
    pub loop_signal: LoopSignal,
    pub stop: bool,

    pub seat_state: SeatState<State>,
    pub seat: Seat<State>,
    pub keyboard: KeyboardHandle<State>,
    pub pointer: PointerHandle<State>,
    pub clock: Clock<Monotonic>,
    pub suppressed_keys: HashSet<Keysym>,
    // We store both the timer and the keysym used to trigger the key action.
    // When we remove the keysym from suppressed keys we stop it.
    pub repeated_keyaction_timer: Option<(RegistrationToken, Keysym)>,
    // To focus a layer-surface with an OnDemand keyboard exclusivity mode, the user must click
    // on the layer surface. Then, when we update the keyboard focus, we check against the clicked
    // layer surface
    pub focused_on_demand_layer_shell: Option<LayerSurface>,

    pub devices: Vec<input::Device>,

    pub dnd_icon: Option<WlSurface>,
    pub cursor_theme_manager: CursorThemeManager,
    pub space: Space,
    pub unmapped_windows: Vec<UnmappedWindow>,
    pub popups: PopupManager,
    pub root_surfaces: HashMap<WlSurface, WlSurface>,
    pub idle_inhibiting_surfaces: Vec<WlSurface>,
    pub mapped_layer_surfaces: HashMap<LayerSurface, MappedLayer>,
    pub lock_state: LockState,

    pub output_state: HashMap<Output, output::OutputState>,
    // Keep track whether we did some transient output changes.
    //
    // This can happen when you use a tool that interacts with the wlr-output-management protocol.
    // When reloading the config, we want to undo those changes.
    pub has_transient_output_changes: bool,

    pub config: Arc<fht_compositor_config::Config>,
    pub cli_config_path: Option<std::path::PathBuf>,
    // The config_ui also tracks the last configuration error, if any.
    pub config_ui: config_ui::ConfigUi,
    // Keep track of whether we already opened/drawed a config_ui on one output.
    // so that we don't "warp it" to another output while its displayed.
    //
    // Example: user has three outputs, he is focused on the output number 2, so
    // the config_ui is displayed there, but lets say the user focuses the third
    // output, by compositor drawing logic, the config_ui will go and warp to
    // the third output.
    //
    // We avoid this by checking this variable.
    pub config_ui_output: Option<Output>,
    // We keep the config watcher around in case the configuration file path changes.
    // This will be useful for configuration file imports (when implemented)
    pub config_watcher: Option<crate::config::Watcher>,

    #[cfg(feature = "dbus")]
    pub dbus_connection: Option<zbus::blocking::Connection>,

    #[cfg(feature = "xdg-screencast-portal")]
    pub pipewire_initialised: std::sync::Once,
    #[cfg(feature = "xdg-screencast-portal")]
    pub pipewire: Option<PipeWire>,

    // Inter-process communication.
    //
    // We keep the IPC server and listener state here. But the actual handling is done inside
    // a Generic calloop source.
    pub ipc_server: Option<ipc::Server>,

    pub compositor_state: CompositorState,
    pub data_control_state: DataControlState,
    pub data_device_state: DataDeviceState,
    pub dmabuf_state: DmabufState,
    pub foreign_toplevel_list_state: ForeignToplevelListState,
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    pub idle_notifier_state: IdleNotifierState<State>,
    pub layer_shell_state: WlrLayerShellState,
    pub output_management_manager_state: OutputManagementManagerState,
    pub primary_selection_state: PrimarySelectionState,
    pub session_lock_manager_state: SessionLockManagerState,
    pub shm_state: ShmState,
    pub xdg_activation_state: XdgActivationState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_foreign_state: XdgForeignState,
}

impl Fht {
    pub fn new(
        dh: &DisplayHandle,
        loop_handle: LoopHandle<'static, State>,
        loop_signal: LoopSignal,
        ipc_server: Option<ipc::Server>,
        config_path: Option<std::path::PathBuf>,
    ) -> Self {
        let (executor, scheduler) =
            calloop::futures::executor().expect("Failed to create scheduler");
        loop_handle
            .insert_source(executor, |_, _, _| {
                // This executor only lives to drive futures, we don't really care about the output.
            })
            .unwrap();

        let mut config_ui = config_ui::ConfigUi::new();
        let (config, paths) = match fht_compositor_config::load(config_path.clone()) {
            Ok((config, paths)) => (config, paths),
            Err(err) => {
                error!(?err, "Failed to load configuration, using default");
                // NOTE: By default we enable animationns, justifying animate = true
                config_ui.show(config_ui::Content::ReloadError { error: err }, true);
                (
                    Default::default(),
                    vec![
                        // We still track the user-provided config path (or the default one)
                        // so that if the user changed and reloaded the config path, we can pick it
                        // up.
                        config_path
                            .clone()
                            .unwrap_or_else(fht_compositor_config::config_path),
                    ],
                )
            }
        };

        let config_watcher = crate::config::init_watcher(paths, &loop_handle)
            .inspect_err(|err| warn!(?err, "Failed to start config file watcher"))
            .ok();

        let clock = Clock::<Monotonic>::new();

        let compositor_state = CompositorState::new_v6::<State>(dh);
        let primary_selection_state = PrimarySelectionState::new::<State>(dh);
        let data_control_state =
            DataControlState::new::<State, _>(dh, Some(&primary_selection_state), |_| true);
        let data_device_state = DataDeviceState::new::<State>(dh);
        let idle_notifier_state = IdleNotifierState::new(dh, loop_handle.clone());
        let foreign_toplevel_list_state = ForeignToplevelListState::new::<State>(dh);
        let dmabuf_state = DmabufState::new();
        let layer_shell_state = WlrLayerShellState::new::<State>(dh);
        let output_management_manager_state =
            OutputManagementManagerState::new::<State, _>(dh, |client| {
                // Only privileded clients
                client
                    .get_data::<ClientState>()
                    .is_none_or(|data| data.security_context.is_none())
            });
        let shm_state =
            ShmState::new::<State>(dh, vec![wl_shm::Format::Xbgr8888, wl_shm::Format::Abgr8888]);
        let session_lock_manager_state = SessionLockManagerState::new::<State, _>(dh, |client| {
            // From: https://wayland.app/protocols/security-context-v1
            // "Compositors should forbid nesting multiple security contexts"
            client
                .get_data::<ClientState>()
                .is_none_or(|data| data.security_context.is_none())
        });
        let xdg_activation_state = XdgActivationState::new::<State>(dh);
        let xdg_shell_state = XdgShellState::new::<State>(dh);
        let xdg_foreign_state = XdgForeignState::new::<State>(dh);
        ContentTypeState::new::<State>(dh);
        CursorShapeManagerState::new::<State>(dh);
        TextInputManagerState::new::<State>(dh);
        InputMethodManagerState::new::<State, _>(dh, |_| true);
        IdleInhibitManagerState::new::<State>(dh);
        VirtualKeyboardManagerState::new::<State, _>(dh, |_| true);
        PointerConstraintsState::new::<State>(dh);
        TabletManagerState::new::<State>(dh);
        SecurityContextState::new::<State, _>(dh, |client| {
            // From: https://wayland.app/protocols/security-context-v1
            // "Compositors should forbid nesting multiple security contexts"
            client
                .get_data::<ClientState>()
                .is_none_or(|data| data.security_context.is_none())
        });
        ScreencopyManagerState::new::<State, _>(dh, |client| {
            // Same idea as security context state.
            client
                .get_data::<ClientState>()
                .is_none_or(|data| data.security_context.is_none())
        });
        XdgDialogState::new::<State>(dh);
        XdgDecorationState::new::<State>(dh);
        FractionalScaleManagerState::new::<State>(dh);
        OutputManagerState::new_with_xdg_output::<State>(dh);
        PresentationState::new::<State>(dh, clock.id() as u32);
        ViewporterState::new::<State>(dh);
        SinglePixelBufferState::new::<State>(dh);
        AlphaModifierState::new::<State>(dh);
        RelativePointerManagerState::new::<State>(dh);

        // Initialize a seat and immediatly attach a keyboard and pointer to it.
        // If clients try to connect and do not find any of them they will try to initialize them
        // themselves and chaos will endure.
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(dh, "seat0");

        // Dont let the user crash the compositor with invalid config
        let keyboard_config = &config.input.keyboard;
        let res = seat.add_keyboard(
            keyboard_config.xkb_config(),
            keyboard_config.repeat_delay.get() as i32,
            keyboard_config.repeat_rate.get(),
        );
        let keyboard = match res {
            Ok(k) => k,
            Err(err) => {
                error!(
                    ?err,
                    "Failed to add keyboard with user xkb config! Falling back to defaults"
                );
                seat.add_keyboard(
                    XkbConfig::default(),
                    keyboard_config.repeat_delay.get() as i32,
                    keyboard_config.repeat_rate.get(),
                )
                .expect("The keyboard is not keyboarding")
            }
        };
        let pointer = seat.add_pointer();
        let cursor_theme_manager = CursorThemeManager::new(config.cursor.clone());
        let keyboard_shortcuts_inhibit_state = KeyboardShortcutsInhibitState::new::<State>(dh);

        #[cfg(feature = "dbus")]
        let dbus_connection = {
            zbus::blocking::Connection::session()
                .and_then(|cnx| {
                    cnx.request_name("fht.desktop.Compositor")?;
                    Ok(cnx)
                })
                .inspect_err(|err| error!(?err, "Failed to connect to session D-Bus"))
                .ok()
        };

        let space = Space::new(&config);

        Self {
            display_handle: dh.clone(),
            loop_handle,
            scheduler,

            loop_signal,
            stop: false,

            clock,
            suppressed_keys: HashSet::new(),
            repeated_keyaction_timer: None,
            seat,
            devices: vec![],
            seat_state,
            keyboard,
            pointer,
            mapped_layer_surfaces: HashMap::new(),
            lock_state: LockState::Unlocked,
            focused_on_demand_layer_shell: None,

            dnd_icon: None,
            cursor_theme_manager,
            space,
            unmapped_windows: vec![],
            popups: PopupManager::default(),
            root_surfaces: HashMap::default(),
            idle_inhibiting_surfaces: Vec::new(),

            output_state: HashMap::new(),
            has_transient_output_changes: false,

            config: Arc::new(config),
            cli_config_path: config_path,
            config_ui,
            config_ui_output: None,
            config_watcher,

            #[cfg(feature = "dbus")]
            dbus_connection,

            #[cfg(feature = "xdg-screencast-portal")]
            pipewire_initialised: std::sync::Once::new(),
            #[cfg(feature = "xdg-screencast-portal")]
            pipewire: None,

            ipc_server,

            compositor_state,
            data_control_state,
            data_device_state,
            dmabuf_state,
            foreign_toplevel_list_state,
            keyboard_shortcuts_inhibit_state,
            idle_notifier_state,
            layer_shell_state,
            output_management_manager_state,
            primary_selection_state,
            shm_state,
            session_lock_manager_state,
            xdg_activation_state,
            xdg_shell_state,
            xdg_foreign_state,
        }
    }

    pub fn add_output(
        &mut self,
        output: Output,
        refresh_interval: Option<Duration>,
        vrr_enabled: bool,
    ) {
        assert!(
            !self.space.has_output(&output),
            "Tried to add an output twice!"
        );

        info!(name = output.name(), "Adding new output");
        self.space.add_output(output.clone());

        let state = output::OutputState {
            redraw_state: output::RedrawState::Idle,
            frame_clock: FrameClock::new(refresh_interval, vrr_enabled),
            animations_running: false,
            current_frame_sequence: 0u32,
            pending_screencopies: vec![],
            screencopy_damage_tracker: None,
            debug_damage_tracker: None,
            lock_surface: None,
            lock_backdrop: None,
        };
        self.output_state.insert(output.clone(), state);

        // Focus output now.
        if self.config.general.cursor_warps {
            let center = output.geometry().center();
            self.loop_handle.insert_idle(move |state| {
                state.move_pointer(center.to_f64());
            });
        }
        self.space.set_active_output(&output);

        // wlr-output-management
        self.output_management_manager_state
            .add_head::<State>(&output);
        self.output_management_manager_state.update::<State>();

        self.arrange_outputs(Some(output));
    }

    pub fn remove_output(&mut self, output: &Output) {
        info!(name = output.name(), "Removing output");
        self.space.remove_output(output);
        self.arrange_outputs(None);
        // wlr-output-management
        self.output_management_manager_state.remove_head(output);
        self.output_management_manager_state.update::<State>();

        // Cleanly close [`LayerSurface`] instead of letting them know their demise after noticing
        // the output is gone.
        for layer in layer_map_for_output(output).layers() {
            layer.layer_surface().send_close()
        }
    }

    pub fn focus_output(&mut self, output: &Output) {
        if let Some(window) = self.space.set_active_output(output) {
            self.loop_handle.insert_idle(move |state| {
                state.set_keyboard_focus(Some(window));
            });

            if self.config.general.cursor_warps {
                let center = output.geometry().center();
                self.loop_handle.insert_idle(move |state| {
                    state.move_pointer(center.to_f64());
                });
            }
        }
    }

    pub fn output_resized(&mut self, output: &Output) {
        crate::profile_function!();

        layer_map_for_output(output).arrange();
        self.space
            .output_resized(output, !self.config.animations.disable);

        #[cfg(feature = "xdg-screencast-portal")]
        {
            // Even though casts should automatically resize, inform the cast stream sooner so that
            // we dont have to some frames to run ensure size in the draw iteration
            if let Some(pipewire) = self.pipewire.as_mut() {
                let cast_source = CastSource::Output(output.downgrade());
                let transform = output.current_transform();
                let size = transform.transform_size(output.current_mode().unwrap().size);

                pipewire
                    .casts
                    .iter_mut()
                    .filter(|cast| *cast.source() == cast_source)
                    .for_each(|cast| {
                        let _ = cast.ensure_size(size);
                    });
            }
        }

        let output_state = self.output_state.get_mut(output).unwrap();
        let _ = output_state.debug_damage_tracker.take();

        if let Some(lock_surface) = output_state.lock_surface.as_ref() {
            // Resize lock surface to make sure it always covers up everything
            lock_surface.with_pending_state(|state| {
                let size = output.geometry().size;
                state.size = Some((size.w as _, size.h as _).into());
            });
            lock_surface.send_configure();
        }

        if let Some(buffer) = &mut output_state.lock_backdrop {
            // Resize lock backdrop to make sure it always covers up everything
            buffer.resize(output.geometry().size);
        }

        let output2 = output.clone();
        self.loop_handle.insert_idle(move |state| {
            state.backend.with_renderer(|renderer| {
                if let Err(err) = EffectsFramebuffers::update_for_output(&output2, renderer) {
                    error!(?err, "Failed to update output effects framebuffers")
                }
            });
        });

        self.queue_redraw(output);
    }

    pub fn reload_output_config(&mut self) {
        // We only care about the outputs that have associated configuration
        //
        // NOTE: Maybe we should 'undo' the configuration of outputs that had a configuration set
        // but got their config removed after? If so, to **what** should we revert it?
        for (output, config) in self
            .space
            .outputs()
            .map(|output| (output, self.config.outputs.get(&output.name())))
        {
            // NOTE: for winit backend the transform must stay on Flipped180.
            let new_transform = (output.name().as_str() != "winit")
                .then(|| config.as_ref().and_then(|cfg| cfg.transform))
                .flatten()
                .map(Into::into)
                .unwrap_or(smithay::utils::Transform::Normal);
            let new_scale = config
                .as_ref()
                .and_then(|cfg| Some(smithay::output::Scale::Integer(cfg.scale?.clamp(1, 10))))
                .unwrap_or(smithay::output::Scale::Integer(1));

            output.change_current_state(None, Some(new_transform), Some(new_scale), None);
        }

        // If we had previous output changes, we force re-apply all config.
        let force = self.has_transient_output_changes;
        let outputs = self.space.outputs().cloned().collect::<Vec<_>>();
        outputs.iter().for_each(|o| self.output_resized(o));
        self.loop_handle.insert_idle(move |state| {
            #[cfg(feature = "udev-backend")]
            #[allow(irrefutable_let_patterns)]
            if let Backend::Udev(udev) = &mut state.backend {
                udev.reload_output_configuration(&mut state.fht, force);
            }
            state.fht.arrange_outputs(None);
        });

        // By now we would have applied everything aligned to our config.
        self.has_transient_output_changes = false;

        // We don't have todo this since it should be done with State::reload_config
        // self.queue_redraw_all();
    }

    pub fn arrange_outputs(&mut self, new_output: Option<Output>) {
        crate::profile_function!();
        let mut outputs = self
            .space
            .outputs()
            .cloned()
            .map(|o| {
                let current_pos = Some(o.current_location());
                let config_pos = self
                    .config
                    .outputs
                    .get(&o.name())
                    .and_then(|c| c.position)
                    .map(|fht_compositor_config::OutputPosition { x, y }| {
                        Point::<i32, Logical>::from((x, y))
                    });
                (o, current_pos, config_pos)
            })
            .collect::<Vec<_>>();
        if let Some(new_output) = &new_output {
            // new output has no initial position!
            if let Some((_, pos, _)) = outputs.iter_mut().find(|(o, _, _)| o == new_output) {
                *pos = None;
            }
        }
        // When we arrange outputs, we must take into consideration the fact that the backend (udev)
        // might make them appear differently since nothing ensures connector order, this is why
        // we order the outputs by their name.
        outputs.sort_unstable_by_key(|(o, _, _)| o.name());
        // First arrange the outputs with an explicit config.
        outputs.sort_unstable_by_key(|(_, _, pos)| pos.is_none());

        let mut arranged_outputs = vec![];
        for (output, current_pos, config_pos) in outputs {
            let size = output.geometry().size;
            let new_pos = config_pos
                .filter(|&target_pos| {
                    let target_geo = Rectangle::new(target_pos, size);
                    // if we have overlap, this position is not good, simple as that.
                    if let Some(overlap) = arranged_outputs
                        .iter()
                        .map(OutputExt::geometry)
                        .find(|geo| geo.overlaps(target_geo))
                    {
                        warn!(
                            "Output {} at {:?} with size {:?} \
                        overlaps an existing output at {:?} with size {:?}! \
                        Using fallback location",
                            output.name(),
                            (target_geo.loc.x, target_geo.loc.y),
                            (target_geo.size.w, target_geo.size.h),
                            (overlap.loc.x, overlap.loc.y),
                            (overlap.size.w, overlap.size.h),
                        );

                        false
                    } else {
                        true
                    }
                })
                .unwrap_or_else(|| {
                    let x_loc = arranged_outputs
                        .iter()
                        .map(OutputExt::geometry)
                        .map(|geo| geo.loc.x + geo.size.w)
                        .max()
                        .unwrap_or(0);
                    Point::from((x_loc, 0))
                });

            if Some(new_pos) != current_pos {
                output.change_current_state(None, None, None, Some(new_pos));
                self.queue_redraw(&output);
            }

            arranged_outputs.push(output);
        }
    }

    pub fn output_update_vrr(&mut self, output: &Output) {
        crate::profile_function!();
        let name = output.name();
        let Some(config) = self.config.outputs.get(&name) else {
            return; // no config, VRR disabled by default.
        };

        let new_state = match config.vrr {
            fht_compositor_config::VrrMode::OnDemand => {
                // We only enable VRR when there's a window scanned out to the prmiary plane
                // with the vrr rule enabled.
                self.space.windows_on_output(output).any(|window| {
                    if window.rules().vrr != Some(true) {
                        return false;
                    }

                    // FIXME: Should we check for subsurfaces too?
                    let wl_surface = window.wl_surface().unwrap();
                    with_states(&wl_surface, |states| {
                        surface_primary_scanout_output(&wl_surface, states).as_ref() == Some(output)
                    })
                })
            }
            _ => return, // Not ondemand, keep it as-is.
        };

        let output = output.clone();
        self.loop_handle.insert_idle(move |state| {
            _ = state
                .backend
                .update_output_vrr(&mut state.fht, &output, new_state);
        });
    }

    pub fn output_named(&self, name: &str) -> Option<Output> {
        if name == "active" {
            Some(self.space.active_output().clone())
        } else {
            self.space.outputs().find(|o| o.name() == name).cloned()
        }
    }

    pub fn queue_redraw(&mut self, output: &Output) {
        let state = self.output_state.get_mut(output).unwrap();
        state.redraw_state.queue();
    }

    pub fn queue_redraw_all(&mut self) {
        for output in self.space.outputs() {
            let state = self.output_state.get_mut(output).unwrap();
            state.redraw_state.queue();
        }
    }

    /// Get the [`PointerFocusTarget`] under a given point and its location in global coordinate
    /// space. We transform elements location from local to global space based on output
    /// location and their position inside the output.
    ///
    /// A focus target is the surface that should get active pointer focus.
    pub fn focus_target_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(PointerFocusTarget, Point<f64, Logical>)> {
        let output = self.space.active_output();
        let output_loc = output.current_location();
        let point_in_output = point - output_loc.to_f64();
        let layer_map = layer_map_for_output(output);

        // If we have a lock surface, return it immediatly
        {
            let output_state = self.output_state.get(output).unwrap();
            if let Some(lock_surface) = &output_state.lock_surface {
                // NOTE: Lock surface is always position at (0,0)
                if let Some((surface, surface_loc)) = under_from_surface_tree(
                    lock_surface.wl_surface(),
                    point_in_output,
                    Point::default(),
                    WindowSurfaceType::ALL,
                ) {
                    return Some((
                        PointerFocusTarget::WlSurface(surface),
                        (surface_loc + output_loc).to_f64(),
                    ));
                }
            }
        }

        let layer_under = |layer| {
            layer_map
                .layer_under(layer, point_in_output)
                .and_then(|layer| {
                    let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
                    layer
                        .surface_under(point_in_output - layer_loc.to_f64(), WindowSurfaceType::ALL)
                        .map(|(surface, surface_loc)| {
                            if surface == *layer.wl_surface() {
                                // Used in handling on-demand layer-shell focus
                                (
                                    PointerFocusTarget::LayerSurface(layer.clone()),
                                    (output_loc + layer_loc).to_f64(),
                                )
                            } else {
                                (
                                    PointerFocusTarget::WlSurface(surface),
                                    (surface_loc + output_loc + layer_loc).to_f64(),
                                )
                            }
                        })
                })
        };

        let window_under = |fullscreen| {
            let maybe_window = if fullscreen {
                self.space.fullscreened_window_under(point)
            } else {
                self.space.window_under(point)
            };

            maybe_window.and_then(|(window, window_loc)| {
                let window_wl_surface = window.wl_surface().unwrap();
                window
                    .surface_under(
                        point_in_output - window_loc.to_f64(),
                        WindowSurfaceType::ALL,
                    )
                    .map(|(surface, surface_loc)| {
                        if surface == *window_wl_surface {
                            // Use the window immediatly when we are the toplevel surface.
                            // PointerFocusTarget::Window to proceed (namely
                            // State::process_mouse_action).
                            (
                                PointerFocusTarget::Window(window.clone()),
                                (window_loc + output_loc).to_f64(),
                            )
                        } else {
                            (
                                PointerFocusTarget::from(surface),
                                (surface_loc + window_loc + output_loc).to_f64(),
                            )
                        }
                    })
            })
        };

        // We must keep these in accordance with rendering order, otherwise there will be
        // inconsistencies with how rendering is done and how input is handled.
        layer_under(Layer::Overlay)
            .or_else(|| window_under(true))
            .or_else(|| layer_under(Layer::Top))
            .or_else(|| window_under(false))
            .or_else(|| layer_under(Layer::Bottom))
            .or_else(|| layer_under(Layer::Background))
    }

    /// Focus the given layer surface if its keyboard interactivity is set to
    /// [`KeyboardInteractivity::OnDemand`], and returns whether it focused it or not
    pub fn set_on_demand_layer_shell_focus(&mut self, layer: Option<&LayerSurface>) {
        if let Some(layer) = layer {
            if layer.cached_state().keyboard_interactivity == KeyboardInteractivity::OnDemand {
                if self.focused_on_demand_layer_shell.as_ref() != Some(layer) {
                    self.focused_on_demand_layer_shell = Some(layer.clone());
                    self.queue_redraw_all(); // FIXME: Granular with layer output
                    return;
                }
            }
        }

        // A layer-shell with keyboard interacitivty set to something else got clicked,
        // remove the select one if any
        if layer.is_some() {
            self.focused_on_demand_layer_shell = None;
            self.queue_redraw_all();
        }
    }

    pub fn visible_output_for_surface(&self, surface: &WlSurface) -> Option<&Output> {
        for output in self.space.outputs() {
            // Lock surface and layer shells take priority.
            let output_state = self.output_state.get(output).unwrap();
            if output_state
                .lock_surface
                .as_ref()
                .is_some_and(|lock_surface| lock_surface.wl_surface() == surface)
            {
                return Some(output);
            }

            let layer_map = layer_map_for_output(output);
            if layer_map
                .layer_for_surface(surface, WindowSurfaceType::ALL)
                .is_some()
            {
                return Some(output);
            }
        }

        self.space.output_for_surface(surface)
    }

    pub fn send_frames(&self, output: &Output) {
        crate::profile_function!();
        let time = self.clock.now();
        let throttle = Some(Duration::from_secs(1));
        let output_state = self.output_state.get(output).unwrap();
        let sequence = output_state.current_frame_sequence;

        let should_send_frames = |surface: &WlSurface, states: &SurfaceData| {
            should_send_frames(output, sequence, surface, states)
        };

        if let Some(lock_surface) = output_state.lock_surface.as_ref() {
            send_frames_surface_tree(
                lock_surface.wl_surface(),
                output,
                time,
                throttle,
                should_send_frames,
            );
        }

        if let CursorImageStatus::Surface(surface) = self.cursor_theme_manager.image_status() {
            send_frames_surface_tree(surface, output, time, throttle, should_send_frames);
        }

        if let Some(surface) = &self.dnd_icon {
            send_frames_surface_tree(surface, output, time, throttle, should_send_frames);
        }

        for window in self.space.visible_windows_for_output(output) {
            window.send_frame(output, time, throttle, should_send_frames);
        }

        let map = layer_map_for_output(output);
        for layer_surface in map.layers() {
            layer_surface.send_frame(output, time, throttle, should_send_frames);
        }
    }

    pub fn update_primary_scanout_output(
        &self,
        output: &Output,
        render_element_states: &RenderElementStates,
    ) {
        crate::profile_function!();
        let output_state = self.output_state.get(output).unwrap();
        if let Some(lock_surface) = output_state.lock_surface.as_ref() {
            with_surface_tree_downward(
                lock_surface.wl_surface(),
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |surface, states, _| {
                    update_surface_primary_scanout_output(
                        surface,
                        output,
                        states,
                        render_element_states,
                        default_primary_scanout_output_compare,
                    );
                },
                |_, _, _| true,
            );
        }

        if let CursorImageStatus::Surface(surface) = self.cursor_theme_manager.image_status() {
            with_surface_tree_downward(
                surface,
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |surface, states, _| {
                    update_surface_primary_scanout_output(
                        surface,
                        output,
                        states,
                        render_element_states,
                        default_primary_scanout_output_compare,
                    );
                },
                |_, _, _| true,
            );
        }

        if let Some(surface) = &self.dnd_icon {
            with_surface_tree_downward(
                surface,
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |surface, states, _| {
                    update_surface_primary_scanout_output(
                        surface,
                        output,
                        states,
                        render_element_states,
                        default_primary_scanout_output_compare,
                    );
                },
                |_, _, _| true,
            );
        }

        // Both windows and layer surfaces can only be drawn on a single output at a time, so there
        // no need to update all the windows of the output.

        for window in self.space.visible_windows_for_output(output) {
            let offscreen_id = window.offscreen_element_id();
            window.with_surfaces(|surface, surface_data| {
                // We do the work of update_surface_primary_scanout_output, but use our own
                // offscreen Id if needed.
                surface_data
                    .data_map
                    .insert_if_missing_threadsafe(Mutex::<PrimaryScanoutOutput>::default);
                let surface_primary_scanout_output = surface_data
                    .data_map
                    .get::<Mutex<PrimaryScanoutOutput>>()
                    .unwrap();
                let id = offscreen_id.clone().unwrap_or_else(|| surface.into());
                let primary_scanout_output = surface_primary_scanout_output
                    .lock()
                    .unwrap()
                    .update_from_render_element_states(
                        id,
                        output,
                        render_element_states,
                        default_primary_scanout_output_compare,
                    );

                if let Some(output) = primary_scanout_output {
                    with_fractional_scale(surface_data, |fraction_scale| {
                        fraction_scale
                            .set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }
            });
        }

        for surface in layer_map_for_output(output).layers() {
            surface.with_surfaces(|surface, states| {
                let primary_scanout_output = update_surface_primary_scanout_output(
                    surface,
                    output,
                    states,
                    render_element_states,
                    // Layer surfaces are shown only on one output at a time.
                    |_, _, output, _| output,
                );

                if let Some(output) = primary_scanout_output {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale
                            .set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }
            });
        }
    }

    pub fn send_dmabuf_feedbacks(
        &self,
        output: &Output,
        feedback: &SurfaceDmabufFeedback,
        render_element_states: &RenderElementStates,
    ) {
        crate::profile_function!();
        let output_state = self.output_state.get(output).unwrap();
        if let Some(lock_surface) = output_state.lock_surface.as_ref() {
            send_dmabuf_feedback_surface_tree(
                lock_surface.wl_surface(),
                output,
                surface_primary_scanout_output,
                |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        render_element_states,
                        &feedback.render_feedback,
                        &feedback.scanout_feedback,
                    )
                },
            );
        }

        if let Some(surface) = &self.dnd_icon {
            send_dmabuf_feedback_surface_tree(
                surface,
                output,
                surface_primary_scanout_output,
                |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        render_element_states,
                        &feedback.render_feedback,
                        &feedback.scanout_feedback,
                    )
                },
            );
        }

        if let CursorImageStatus::Surface(surface) = self.cursor_theme_manager.image_status() {
            send_dmabuf_feedback_surface_tree(
                surface,
                output,
                surface_primary_scanout_output,
                |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        render_element_states,
                        &feedback.render_feedback,
                        &feedback.scanout_feedback,
                    )
                },
            );
        }

        for window in self.space.visible_windows_for_output(output) {
            window.send_dmabuf_feedback(
                output,
                |_, _| Some(output.clone()),
                |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        render_element_states,
                        &feedback.render_feedback,
                        &feedback.scanout_feedback,
                    )
                },
            );
        }

        for surface in layer_map_for_output(output).layers() {
            surface.send_dmabuf_feedback(
                output,
                |_, _| Some(output.clone()),
                |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        render_element_states,
                        &feedback.render_feedback,
                        &feedback.scanout_feedback,
                    )
                },
            );
        }
    }

    pub fn take_presentation_feedback(
        &self,
        output: &Output,
        render_element_states: &RenderElementStates,
    ) -> OutputPresentationFeedback {
        crate::profile_function!();
        let mut output_presentation_feedback = OutputPresentationFeedback::new(output);
        let output_state = self.output_state.get(output).unwrap();

        if let Some(lock_surface) = output_state.lock_surface.as_ref() {
            take_presentation_feedback_surface_tree(
                lock_surface.wl_surface(),
                &mut output_presentation_feedback,
                surface_primary_scanout_output,
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(surface, render_element_states)
                },
            );
        }

        if let CursorImageStatus::Surface(surface) = self.cursor_theme_manager.image_status() {
            take_presentation_feedback_surface_tree(
                surface,
                &mut output_presentation_feedback,
                surface_primary_scanout_output,
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(surface, render_element_states)
                },
            );
        }

        if let Some(surface) = &self.dnd_icon {
            take_presentation_feedback_surface_tree(
                surface,
                &mut output_presentation_feedback,
                surface_primary_scanout_output,
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(surface, render_element_states)
                },
            );
        }

        for window in self.space.visible_windows_for_output(output) {
            window.take_presentation_feedback(
                &mut output_presentation_feedback,
                surface_primary_scanout_output,
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(surface, render_element_states)
                },
            )
        }

        let map = layer_map_for_output(output);
        for layer_surface in map.layers() {
            layer_surface.take_presentation_feedback(
                &mut output_presentation_feedback,
                surface_primary_scanout_output,
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(surface, render_element_states)
                },
            );
        }

        output_presentation_feedback
    }

    pub fn resolve_rules_for_window(&self, window: &Window) {
        crate::profile_function!();
        for monitor in self.space.monitors() {
            let output_name = monitor.output().name();
            for (ws_idx, workspace) in monitor.workspaces().enumerate() {
                let focused_idx = workspace.active_tile_idx();
                for (window_idx, other_window) in workspace.windows().enumerate() {
                    if window == other_window {
                        let rules = ResolvedWindowRules::resolve(
                            window,
                            &self.config.rules,
                            &output_name,
                            ws_idx,
                            focused_idx.is_some_and(|idx| idx == window_idx),
                        );
                        window.set_rules(rules);

                        return;
                    }
                }
            }
        }
    }

    pub fn refresh_ipc(&mut self) {
        let Some(ipc::Server {
            compositor_state, ..
        }) = &mut self.ipc_server
        else {
            return;
        };

        let keyboard_focus = self.keyboard.current_focus();
        let is_focused = move |window: &Window| matches!(&keyboard_focus, Some(KeyboardFocusTarget::Window(w)) if w == window);

        let mut events = vec![];

        let mut existing_windows = HashSet::new();
        for monitor in self.space.monitors() {
            let output_name = monitor.output().name();
            for workspace in monitor.workspaces() {
                let ws_id = workspace.id();
                let active_tile_idx = workspace.active_tile_idx();

                let make_ipc_window = |idx, tile: &space::Tile| {
                    let window = tile.window();
                    let location = tile.location() + tile.window_loc();
                    let size = window.size();

                    fht_compositor_ipc::Window {
                        id: *window.id(),
                        title: window.title(),
                        app_id: window.app_id(),
                        workspace_id: *ws_id,
                        size: (size.w as u32, size.h as u32),
                        location: location.into(),
                        fullscreened: window.fullscreen(),
                        maximized: window.maximized(),
                        tiled: window.tiled(),
                        activated: Some(idx) == active_tile_idx,
                        focused: is_focused(window),
                    }
                };

                // First diff windows.
                for (idx, tile) in workspace.tiles().enumerate() {
                    let id = tile.window().id();
                    existing_windows.insert(*id);

                    let entry = compositor_state.windows.entry(*id);
                    entry
                        .and_modify(|window| {
                            let location = tile.location() + tile.window_loc();
                            let size = tile.window().size();

                            let mut changed = false;
                            // FIXME: This is quite a lot of checking.
                            changed |= tile.window().title() != window.title;
                            changed |= tile.window().app_id() != window.app_id;
                            changed |= tile.window().maximized() != window.maximized;
                            changed |= tile.window().fullscreen() != window.fullscreened;
                            changed |= tile.window().tiled() != window.tiled;
                            changed |= *ws_id != window.workspace_id;
                            changed |= window.location.0 != location.x;
                            changed |= window.location.1 != location.y;
                            changed |= window.size.0 != size.w as u32;
                            changed |= window.size.1 != size.h as u32;

                            if changed {
                                *window = make_ipc_window(idx, tile);
                                events
                                    .push(fht_compositor_ipc::Event::WindowChanged(window.clone()));
                            }
                        })
                        .or_insert_with(|| {
                            let new_window = make_ipc_window(idx, tile);
                            events
                                .push(fht_compositor_ipc::Event::WindowChanged(new_window.clone()));
                            new_window
                        });
                }

                // Then diff the workspace.
                let entry = compositor_state.workspaces.entry(*ws_id);
                entry
                    .and_modify(|ipc_ws| {
                        let mut changed = false;
                        changed |= output_name != ipc_ws.output;

                        let current_windows: Vec<_> =
                            workspace.windows().map(Window::id).map(|id| *id).collect();
                        changed |= current_windows != ipc_ws.windows;
                        changed |= workspace.active_tile_idx() != ipc_ws.active_window_idx;
                        changed |=
                            workspace.fullscreened_tile_idx() != ipc_ws.fullscreen_window_idx;
                        changed |= workspace.mwfact() != ipc_ws.mwfact;
                        changed |= workspace.nmaster() != ipc_ws.nmaster;

                        if changed {
                            let new_workspace = fht_compositor_ipc::Workspace {
                                id: *ws_id,
                                output: output_name.clone(),
                                windows: current_windows,
                                active_window_idx: workspace.active_tile_idx(),
                                fullscreen_window_idx: workspace.fullscreened_tile_idx(),
                                mwfact: workspace.mwfact(),
                                nmaster: workspace.nmaster(),
                            };
                            *ipc_ws = new_workspace;
                            events
                                .push(fht_compositor_ipc::Event::WorkspaceChanged(ipc_ws.clone()));
                        }
                    })
                    .or_insert_with(|| {
                        let current_windows: Vec<_> =
                            workspace.windows().map(Window::id).map(|id| *id).collect();
                        let new_workspace = fht_compositor_ipc::Workspace {
                            id: *ws_id,
                            output: output_name.clone(),
                            windows: current_windows,
                            active_window_idx: workspace.active_tile_idx(),
                            fullscreen_window_idx: workspace.fullscreened_tile_idx(),
                            mwfact: workspace.mwfact(),
                            nmaster: workspace.nmaster(),
                        };
                        events.push(fht_compositor_ipc::Event::WorkspaceChanged(
                            new_workspace.clone(),
                        ));
                        new_workspace
                    });
            }
        }
        // Now remove old windows.
        let all_ids: Vec<_> = compositor_state.windows.keys().copied().collect();
        for id in all_ids {
            if !existing_windows.contains(&id) {
                _ = compositor_state.windows.remove(&id);
                events.push(fht_compositor_ipc::Event::WindowClosed { id })
            }
        }

        let Some(server) = &mut self.ipc_server else {
            unreachable!()
        };

        if let Err(err) = server.push_events(events, &self.scheduler) {
            error!(?err, "Failed to broadcast IPC events");
        };
    }

    pub fn refresh_idle_inhibit(&mut self) {
        self.idle_inhibiting_surfaces.retain(|s| s.alive());
        let is_inhibited = self.idle_inhibiting_surfaces.iter().any(|surface| {
            with_states(surface, |states| {
                // only inhibit if its scanned out
                surface_primary_scanout_output(surface, states).is_some()
            })
        });
        self.idle_notifier_state.set_is_inhibited(is_inhibited);
    }

    pub fn resolve_rules_for_all_windows_if_needed(&self) {
        crate::profile_function!();
        for monitor in self.space.monitors() {
            let output_name = monitor.output().name();
            for (ws_idx, workspace) in monitor.workspaces().enumerate() {
                let Some(focused_idx) = workspace.active_tile_idx() else {
                    continue; // No windows on the workspace, do not bother.
                };
                for (window_idx, window) in workspace.windows().enumerate() {
                    if !window.need_to_resolve_rules() {
                        continue;
                    }
                    let rules = ResolvedWindowRules::resolve(
                        window,
                        &self.config.rules,
                        &output_name,
                        ws_idx,
                        window_idx == focused_idx,
                    );
                    window.set_rules(rules);
                }
            }
        }
    }

    pub fn add_libinput_device(&mut self, mut device: input::Device) {
        // The following input configuration logic is from hyprland.
        let input_config = &self.config.input;
        let per_device_config = input_config
            .per_device
            .get(device.name())
            .or_else(|| input_config.per_device.get(device.sysname()));

        self.keyboard.change_repeat_info(
            input_config.keyboard.repeat_rate.get(),
            input_config.keyboard.repeat_delay.get() as i32,
        );

        let disable = per_device_config.is_some_and(|c| c.disable);
        // The device is disabled, no need to apply any configuration
        if disable {
            let _ = device.config_send_events_set_mode(SendEventsMode::DISABLED);
        } else {
            let _ = device.config_send_events_set_mode(SendEventsMode::ENABLED);

            // Aquamarine (hyprland's input backend) determines a libinput device is a mouse by
            // the pointer capability:
            // https://github.com/hyprwm/aquamarine/blob/752d0fbd141fabb5a1e7f865199b80e6e76f8d8e/src/backend/Session.cpp#L826
            if device.has_capability(DeviceCapability::Pointer) {
                // A pointer with a size is a touchpad
                let is_touchpad = device.size().is_some_and(|(w, h)| w != 0. && h != 0.);
                // Trackpoints are reported as pointingsticks in udev
                // https://wayland.freedesktop.org/libinput/doc/latest/trackpoint-configuration.html
                // And based on udev source, here's the property value we must search for
                // https://github.com/systemd/systemd/blob/d38dd7d17a67fda3257905fa32f254cd7b7d5b83/src/udev/udev-builtin-input_id.c#L315
                #[cfg(feature = "udev-backend")]
                let is_trackpoint = unsafe { device.udev_device() }.is_some_and(|device| {
                    device.property_value("ID_INPUT_POINTINGSTICK").is_some()
                });
                #[cfg(not(feature = "udev-backend"))]
                // When we are with winit backend, who cares, this function won't be called anyway
                // since there won't be any sort of libinput devices registered
                let is_trackpoint = false;

                let mouse_config = per_device_config.map_or_else(
                    || match (is_touchpad, is_trackpoint) {
                        // Not a touchpad and not a trackpoint is just a generic mouse.
                        (false, false) => &input_config.mouse,
                        (true, false) => &input_config.touchpad,
                        (false, true) => &input_config.trackpoint,
                        _ => unreachable!(),
                    },
                    |cfg|
                    // In the case we use the per-device config, the user already knows what device he's modifying,
                    // so we just use the mouse attribute.
                    &cfg.mouse,
                );

                if let Some(click_method) = mouse_config.click_method {
                    let _ = device.config_click_set_method(click_method.into());
                } else if let Some(default) = device.config_click_default_method() {
                    let _ = device.config_click_set_method(default);
                }

                if device.config_left_handed_default() {
                    if let Some(left_handed) = mouse_config.left_handed {
                        let _ = device.config_left_handed_set(left_handed);
                    } else {
                        let default = device.config_left_handed_default();
                        let _ = device.config_left_handed_set(default);
                    }
                }

                if let Some(scroll_method) = mouse_config.scroll_method {
                    let _ = device.config_scroll_set_method(scroll_method.into());
                } else if let Some(default) = device.config_scroll_default_method() {
                    let _ = device.config_scroll_set_method(default);
                }

                if let Some(tap_and_drag) = mouse_config.tap_and_drag {
                    let _ = device.config_tap_set_drag_enabled(tap_and_drag);
                } else {
                    let default = device.config_tap_default_drag_enabled();
                    let _ = device.config_tap_set_drag_enabled(default);
                }

                if let Some(drag_lock) = mouse_config.drag_lock {
                    let _ = device.config_tap_set_drag_lock_enabled(drag_lock);
                } else {
                    let default = device.config_tap_default_drag_lock_enabled();
                    let _ = device.config_tap_set_drag_lock_enabled(default);
                }

                if device.config_middle_emulation_is_available() {
                    if let Some(middle_button_emulation) = mouse_config.middle_button_emulation {
                        let _ = device.config_middle_emulation_set_enabled(middle_button_emulation);
                    } else {
                        let default = device.config_middle_emulation_default_enabled();
                        let _ = device.config_middle_emulation_set_enabled(default);
                    }

                    if let Some(tap_button_map) = mouse_config.tap_button_map {
                        let _ = device.config_tap_set_button_map(tap_button_map.into());
                    } else if let Some(default) = device.config_tap_default_button_map() {
                        let _ = device.config_tap_set_button_map(default);
                    }
                }

                if device.config_tap_finger_count() > 0 {
                    if let Some(tap_to_click) = mouse_config.tap_to_click {
                        let _ = device.config_tap_set_enabled(tap_to_click);
                    } else {
                        let default = device.config_tap_default_enabled();
                        let _ = device.config_tap_set_enabled(default);
                    }
                }

                if device.config_scroll_has_natural_scroll() {
                    if let Some(natural_scrolling) = mouse_config.natural_scrolling {
                        let _ = device.config_scroll_set_natural_scroll_enabled(natural_scrolling);
                    } else {
                        let default = device.config_scroll_default_natural_scroll_enabled();
                        let _ = device.config_scroll_set_natural_scroll_enabled(default);
                    }
                }

                if device.config_dwt_is_available() {
                    if let Some(dwt) = mouse_config.disable_while_typing {
                        let _ = device.config_dwt_set_enabled(dwt);
                    } else {
                        let default = device.config_dwt_default_enabled();
                        let _ = device.config_dwt_set_enabled(default);
                    }
                }

                if let Some(speed) = mouse_config.acceleration_speed {
                    let speed = speed.clamp(-1.0, 1.0);
                    let _ = device.config_accel_set_speed(speed);
                } else {
                    let default = device.config_accel_default_speed();
                    let _ = device.config_accel_set_speed(default);
                }

                if let Some(profile) = mouse_config.acceleration_profile {
                    let _ = device.config_accel_set_profile(profile.into());
                } else if let Some(default) = device.config_accel_default_profile() {
                    let _ = device.config_accel_set_profile(default);
                }

                if let Some(scroll_button_lock) = mouse_config.scroll_button_lock {
                    let _ = device.config_scroll_set_button_lock(match scroll_button_lock {
                        true => input::ScrollButtonLockState::Enabled,
                        false => input::ScrollButtonLockState::Disabled,
                    });
                } else {
                    let default = device.config_scroll_default_button_lock();
                    let _ = device.config_scroll_set_button_lock(default);
                }

                if let Some(scroll_button) = mouse_config.scroll_button {
                    let _ = device.config_scroll_set_button(scroll_button.button_code());
                } else {
                    let default = device.config_scroll_default_button();
                    let _ = device.config_scroll_set_button(default);
                }
            }
        }

        self.devices.push(device);
    }

    #[cfg(feature = "xdg-screencast-portal")]
    pub fn stop_cast(&mut self, id: CastId) {
        crate::profile_function!();
        let Some(pipewire) = self.pipewire.as_mut() else {
            return;
        };

        let Some(dbus_conn) = self.dbus_connection.as_mut() else {
            return;
        };

        let Some(idx) = pipewire.casts.iter().position(|c| c.id() == id) else {
            warn!("Tried to stop an invalid cast");
            return;
        };

        let cast = pipewire.casts.swap_remove(idx);
        self.loop_handle.remove(cast.to_compositor_token); // remove calloop stream
        let _ = cast.stream.disconnect(); // even if this fails we dont use the stream anymore

        let object_server = dbus_conn.object_server();
        let Ok(interface) = object_server.interface::<_, ScreencastSession>(&cast.session_handle)
        else {
            warn!(?id, "Cast session doesn't exist");
            return;
        };

        async_io::block_on(async {
            if let Err(err) = interface
                .get()
                .closed(interface.signal_emitter(), std::collections::HashMap::new())
                .await
            {
                warn!(?err, "Failed to send closed signal to screencast session");
            };
        });
    }
}

/// Function to send frame callbacks for a single [`Window`] on the [`Output`].
///
/// This is used in the case of screencasting windows that are not visible on the active
/// workspace. Let's say you are screencasting a window from workspace 3 but you are currently
/// on workspace 1, the only result you will get is the last displayed frame since the window
/// didn't receive frame callbacks.
///
/// In [`Fht::render_screencast_windows`] we make use of this function to avoid such behaviour
#[cfg(feature = "xdg-screencast-portal")]
pub fn send_frame_for_screencast_window(
    output: &Output,
    output_state: &HashMap<Output, output::OutputState>,
    window: &Window,
    target_presentation_time: Duration,
) {
    crate::profile_function!();
    let throttle = Some(Duration::from_secs(1));
    let output_state = output_state.get(output).unwrap();
    let sequence = output_state.current_frame_sequence;

    let should_send_frames = |surface: &WlSurface, states: &SurfaceData| {
        should_send_frames(output, sequence, surface, states)
    };

    window.send_frame(
        output,
        target_presentation_time,
        throttle,
        should_send_frames,
    );
}

/// Check whether we should send frame callbacks to [`surface`] that is displayed on [`Output`].
///
/// This function ensures that the sequencing of frame callbacks that is maintained by the backend
/// is respected, avoiding sending two frame callbacks on a single frame.
///
/// Read [`OutputState::current_frame_sequence`](output::OutputState::current_frame_sequence)
fn should_send_frames(
    output: &Output,
    sequence: u32,
    surface: &WlSurface,
    states: &SurfaceData,
) -> Option<Output> {
    // Use smithay's surface_primary_scanout_output helper to avoid sending frames to
    // invisible surfaces of the output, at the cost of sending more frames for the cursor.
    let current_primary_output = surface_primary_scanout_output(surface, states);
    if current_primary_output.as_ref() != Some(output) {
        return None;
    }

    let last_callback_output: &RefCell<Option<(Output, u32)>> =
        states.data_map.get_or_insert(RefCell::default);
    let mut last_callback_output = last_callback_output.borrow_mut();

    let mut send = true;
    if let Some((last_output, last_sequence)) = last_callback_output.as_ref() {
        // We already sent a frame callback to this surface, do not waste time sending
        if last_output == output && *last_sequence == sequence {
            send = false;
        }
    }

    if send {
        *last_callback_output = Some((output.clone(), sequence));
        Some(output.clone())
    } else {
        None
    }
}

#[derive(Debug, Clone)]
pub struct SurfaceDmabufFeedback {
    pub render_feedback: DmabufFeedback,
    pub scanout_feedback: DmabufFeedback,
}

#[derive(Default, Debug)]
pub struct ClientState {
    pub compositor: CompositorClientState,
    pub security_context: Option<SecurityContext>,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: smithay::reexports::wayland_server::backend::ClientId) {}
    fn disconnected(
        &self,
        _client_id: smithay::reexports::wayland_server::backend::ClientId,
        _reason: smithay::reexports::wayland_server::backend::DisconnectReason,
    ) {
    }
}

// We track ourselves window configure state since some clients may set initial_configure_sent to
// true even if its NOT (example: electron + ozone wayland)
pub enum UnmappedWindow {
    Unconfigured(Window),
    Configured {
        // A big different between an unconfigured and configured unmapped window is that the
        // configured window will have a resolved set of window rules.
        window: Window,
        workspace_id: WorkspaceId,
    },
}

impl UnmappedWindow {
    pub fn window(&self) -> &Window {
        match self {
            Self::Unconfigured(window) => window,
            Self::Configured { window, .. } => window,
        }
    }

    pub fn configured(&self) -> bool {
        matches!(self, Self::Configured { .. })
    }
}

// Resolved window rules that get computed from the configuration.
// They keep around actual values the user specified.
//
// Resolving window rules is combined, as in the it will apply all the matching rules from the
// config only the resolved window rule set.
#[derive(Debug, Clone, Default)]
pub struct ResolvedWindowRules {
    // Border overrides gets applied to the border config when we need the window-specific border
    // config with rules applied (for example when rendering)
    pub border: BorderOverrides,
    pub blur: BlurOverrides,
    pub shadow: ShadowOverrides,
    pub open_on_output: Option<String>,
    pub open_on_workspace: Option<usize>,
    pub opacity: Option<f32>,
    pub proportion: Option<f64>,
    pub decoration_mode: Option<DecorationMode>,
    pub maximized: Option<bool>,
    pub fullscreen: Option<bool>,
    pub floating: Option<bool>,
    pub ontop: Option<bool>,
    pub centered: Option<bool>,
    pub centered_in_parent: Option<bool>,
    pub vrr: Option<bool>,
}

impl ResolvedWindowRules {
    pub fn resolve(
        window: &Window,
        rules: &[fht_compositor_config::WindowRule],
        current_output: &str,
        current_workspace_idx: usize,
        is_focused: bool,
    ) -> Self {
        crate::profile_function!();
        let mut resolved_rules = ResolvedWindowRules::default();

        // NOTE: Bypass for fht-share-picker since it's better when floating centered.
        if window.app_id().as_deref() == Some("fht.desktop.SharePicker") {
            return Self {
                floating: Some(true),
                centered: Some(true),
                ..resolved_rules
            };
        }

        for rule in rules.iter().filter(|rule| {
            rule_matches(
                rule,
                window,
                current_output,
                current_workspace_idx,
                is_focused,
                !window.tiled(),
            )
        }) {
            resolved_rules.border = resolved_rules.border.merge_with(rule.border);
            resolved_rules.blur = resolved_rules.blur.merge_with(rule.blur);
            resolved_rules.shadow = resolved_rules.shadow.merge_with(&rule.shadow);

            if let Some(open_on_output) = &rule.open_on_output {
                resolved_rules.open_on_output = Some(open_on_output.clone())
            }

            if let Some(open_on_workspace) = &rule.open_on_workspace {
                resolved_rules.open_on_workspace = Some(*open_on_workspace)
            }

            if let Some(opacity) = rule.opacity {
                resolved_rules.opacity = Some(opacity)
            }

            if let Some(proportion) = rule.proportion {
                resolved_rules.proportion = Some(proportion)
            }

            if let Some(decoration_mode) = rule.decoration_mode {
                resolved_rules.decoration_mode = Some(decoration_mode)
            }

            if let Some(maximized) = rule.maximized {
                resolved_rules.maximized = Some(maximized)
            }

            if let Some(fullscreen) = rule.fullscreen {
                resolved_rules.fullscreen = Some(fullscreen)
            }

            if let Some(floating) = rule.floating {
                resolved_rules.floating = Some(floating);
            }

            if let Some(centered) = rule.centered {
                resolved_rules.centered = Some(centered);
            }

            if let Some(ontop) = rule.ontop {
                resolved_rules.ontop = Some(ontop);
            }

            if let Some(vrr) = rule.vrr {
                resolved_rules.vrr = Some(vrr);
            }
        }

        resolved_rules
    }
}

fn rule_matches(
    rule: &fht_compositor_config::WindowRule,
    window: &Window,
    current_output: &str,
    current_workspace_idx: usize,
    is_focused: bool,
    is_floating: bool,
) -> bool {
    if rule.match_all {
        // When the user wants to match all the match criteria onto the window, there's two
        // considerations to be done
        // - Only specified criteria should be matched
        // - If the window does not have a app_id and title, the match_title and match_app_id will
        //   be skipped (for not being relevant, maybe not matching would be better?)
        if let Some(window_title) = window.title() {
            if !rule
                .match_title
                .iter()
                .any(|regex| regex.is_match(&window_title))
            {
                return false;
            }
        }

        if let Some(window_app_id) = window.app_id() {
            if !rule
                .match_app_id
                .iter()
                .any(|regex| regex.is_match(&window_app_id))
            {
                return false;
            }
        }

        if let Some(rule_output) = rule.on_output.as_ref() {
            if rule_output != current_output {
                return false;
            }
        }

        if let Some(on_workspace) = rule.on_workspace {
            if on_workspace != current_workspace_idx {
                return false;
            }
        }

        if let Some(rule_is_focused) = rule.is_focused {
            if rule_is_focused != is_focused {
                return false;
            }
        }

        if let Some(rule_is_floating) = rule.is_floating {
            if rule_is_floating != is_floating {
                return false;
            }
        }

        true
    } else {
        if let Some(window_title) = window.title() {
            if rule
                .match_title
                .iter()
                .any(|regex| regex.is_match(&window_title))
            {
                return true;
            }
        }

        if let Some(window_app_id) = window.app_id() {
            if rule
                .match_app_id
                .iter()
                .any(|regex| regex.is_match(&window_app_id))
            {
                return true;
            }
        }

        if let Some(rule_output) = rule.on_output.as_ref() {
            if *rule_output == current_output {
                return true;
            }
        }

        if let Some(on_workspace) = rule.on_workspace {
            if on_workspace == current_workspace_idx {
                return true;
            }
        }

        if let Some(rule_is_focused) = rule.is_focused {
            if rule_is_focused == is_focused {
                return true;
            }
        }

        if let Some(rule_is_floating) = rule.is_floating {
            if rule_is_floating == is_floating {
                return true;
            }
        }

        false
    }
}
