use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use fht_compositor_config::{BorderOverrides, DecorationMode};
use smithay::backend::renderer::element::utils::select_dmabuf_feedback;
use smithay::backend::renderer::element::{
    default_primary_scanout_output_compare, PrimaryScanoutOutput, RenderElementStates,
};
use smithay::desktop::utils::{
    send_dmabuf_feedback_surface_tree, send_frames_surface_tree,
    surface_presentation_feedback_flags_from_states, surface_primary_scanout_output,
    take_presentation_feedback_surface_tree, update_surface_primary_scanout_output,
    OutputPresentationFeedback,
};
use smithay::desktop::{layer_map_for_output, PopupManager};
use smithay::input::keyboard::{KeyboardHandle, Keysym, XkbConfig};
use smithay::input::pointer::{CursorImageStatus, PointerHandle};
use smithay::input::{Seat, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::{LoopHandle, LoopSignal};
use smithay::reexports::input::{self, DeviceCapability, SendEventsMode};
use smithay::reexports::wayland_server::backend::ClientData;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::{Clock, IsAlive, Monotonic};
use smithay::wayland::alpha_modifier::AlphaModifierState;
use smithay::wayland::compositor::{
    with_surface_tree_downward, CompositorClientState, CompositorState, SurfaceData,
    TraversalAction,
};
use smithay::wayland::content_type::ContentTypeState;
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::dmabuf::{DmabufFeedback, DmabufState};
use smithay::wayland::foreign_toplevel_list::ForeignToplevelListState;
use smithay::wayland::fractional_scale::{with_fractional_scale, FractionalScaleManagerState};
use smithay::wayland::input_method::InputMethodManagerState;
use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::pointer_constraints::PointerConstraintsState;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::security_context::{SecurityContext, SecurityContextState};
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::session_lock::{LockSurface, SessionLockManagerState};
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
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
use crate::cli;
use crate::config::ui as config_ui;
use crate::frame_clock::FrameClock;
use crate::handlers::LockState;
use crate::output::{self, OutputExt, RedrawState};
#[cfg(feature = "xdg-screencast-portal")]
use crate::portals::screencast::{
    self, CursorMode, PortalSession, ScreencastSource, StreamMetadata,
};
use crate::protocols::screencopy::ScreencopyManagerState;
use crate::shell::cursor::CursorThemeManager;
use crate::shell::KeyboardFocusTarget;
use crate::space::{Space, WorkspaceId};
#[cfg(feature = "xdg-screencast-portal")]
use crate::utils::dbus::DBUS_CONNECTION;
#[cfg(feature = "xdg-screencast-portal")]
use crate::utils::pipewire::{CastId, CastSource, PipeWire, PwToCompositor};
use crate::utils::RectCenterExt;
use crate::window::Window;

pub struct State {
    pub fht: Fht,
    pub backend: Backend,
}

impl State {
    pub fn new(
        dh: &DisplayHandle,
        loop_handle: LoopHandle<'static, State>,
        loop_signal: LoopSignal,
        cli: cli::Cli,
        _socket_name: String,
    ) -> Self {
        #[allow(unused)]
        let mut fht = Fht::new(dh, loop_handle, loop_signal, cli.config_path);
        #[allow(unused)]
        let backend: crate::backend::Backend = if let Some(backend_type) = cli.backend {
            match backend_type {
                #[cfg(feature = "winit-backend")]
                cli::BackendType::Winit => crate::backend::winit::WinitData::new(&mut fht)
                    .unwrap()
                    .into(),
                #[cfg(feature = "udev-backend")]
                cli::BackendType::Udev => crate::backend::udev::UdevData::new(&mut fht)
                    .unwrap()
                    .into(),
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
        self.fht.resolve_rules_for_all_windows_if_needed();

        {
            crate::profile_scope!("refresh_and_redraw_outputs");
            let mut outputs_to_redraw = vec![];
            let locked = self.fht.is_locked();
            for output in self.fht.space.outputs() {
                let output_state = self.fht.output_state.get_mut(output).unwrap();
                if locked {
                    // Take away the lock surface
                    output_state.has_lock_backdrop = false;
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
        self.fht.lock_state =
            match std::mem::take(&mut self.fht.lock_state) {
                // Switch from pending to locked when we finished drawing a backdrop at least once.
                LockState::Pending(locker)
                    if self.fht.space.outputs().all(|output| {
                        self.fht.output_state.get(output).unwrap().has_lock_backdrop
                    }) =>
                {
                    locker.lock();
                    LockState::Locked
                }
                state => state,
            };

        {
            crate::profile_scope!("refresh_focus");
            // Make sure the surface is not dead (otherwise wayland wont be happy)
            // NOTE: focus_target from state is always guaranteed to be the same as keyboard focus.
            if self.fht.is_locked() {
                // If we are locked, locked surface of active output gets precedence before
                // everything. This also includes pointer focus too.
                //
                // For example, the prompt of your lock screen might need keyboard input.
                let active_output = self.fht.space.active_output().clone();
                let output_state = self.fht.output_state.get(&active_output).unwrap();
                if let Some(lock_surface) = output_state.lock_surface.clone() {
                    // Focus new surface if its different to avoid spamming wl_keyboard::enter event
                    let new_focus = KeyboardFocusTarget::LockSurface(lock_surface);
                    if self.fht.keyboard.current_focus().as_ref() != Some(&new_focus) {
                        self.set_keyboard_focus(Some(new_focus));
                    }
                } else {
                    // We do not have a lock surface on active output, default to not focusing
                    // anything.
                    self.set_keyboard_focus(Option::<LockSurface>::None);
                }
            } else {
                // We are focusing nothing, default to the active workspace focused window.
                let old_focus_dead = self
                    .fht
                    .focus_state
                    .keyboard_focus
                    .as_ref()
                    .is_some_and(|ft| !ft.alive());
                {
                    if old_focus_dead {
                        self.set_keyboard_focus(self.fht.space.active_window());
                    }
                }
            }
        }

        {
            crate::profile_scope!("flush_clients");
            self.fht
                .display_handle
                .flush_clients()
                .context("Failed to flush_clients!")?;
        }

        Ok(())
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

            let monitor = self
                .fht
                .space
                .monitor_mut_for_output(&output)
                .expect("all outputs should be tracked by Space");
            ongoing |= monitor.advance_animations(target_presentation_time);

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
        // let old_config = Arc::clone(&self.fht.config);
        let config = Arc::new(new_config);

        // Some invariants must be upheld when reloading the configuration
        // If any reloading function errors out, the configuration is not valid

        let keyboard = self.fht.keyboard.clone();
        if let Err(err) = keyboard.set_xkb_config(self, config.input.keyboard.xkb_config()) {
            error!(?err, "Failed to apply configuration");
            return;
        }

        // NOTE: A tricky problem here is that a workspace set *can* apply the configuration just
        // file but then one after it fails to apply it. Really confusing behaviour.
        //
        // Maybe we need to store the last working config if this happens
        if let Err(err) = crate::space::Config::check_invariants(&config) {
            error!(?err, "Failed to apply configuration");
        }
        self.fht.space.reload_config(&config);

        self.fht
            .cursor_theme_manager
            .reload_config(config.cursor.clone());

        // If we made it up to here, the configuration must be valid
        self.fht.config = config;

        // These devices are just handles, so cleaning the devices vector and adding them all
        // back should not be an issue. (input device configuration code in inside
        // add_libinput_device function)
        let devices: Vec<_> = self.fht.devices.drain(..).collect();
        for device in devices {
            self.fht.add_libinput_device(device);
        }
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
                    let _ = metadata_sender.send(None);
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

            let Some(gbm_device) = data.devices.get(&data.primary_node).map(|d| d.gbm.clone())
            else {
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
                .with_renderer(|renderer| renderer.egl_context().dmabuf_render_formats().clone());

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
    pub loop_signal: LoopSignal,
    pub stop: bool,

    pub seat_state: SeatState<State>,
    pub seat: Seat<State>,
    pub keyboard: KeyboardHandle<State>,
    pub pointer: PointerHandle<State>,
    pub clock: Clock<Monotonic>,
    pub suppressed_keys: HashSet<Keysym>,
    pub devices: Vec<input::Device>,
    pub interactive_grab_active: bool,
    pub resize_grab_active: bool,

    pub dnd_icon: Option<WlSurface>,
    pub cursor_theme_manager: CursorThemeManager,
    pub space: Space,
    pub unmapped_windows: Vec<UnmappedWindow>,
    pub focus_state: FocusState,
    pub popups: PopupManager,
    pub root_surfaces: HashMap<WlSurface, WlSurface>,
    pub lock_state: LockState,

    pub output_state: HashMap<Output, output::OutputState>,

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

    #[cfg(feature = "xdg-screencast-portal")]
    pub pipewire_initialised: std::sync::Once,
    #[cfg(feature = "xdg-screencast-portal")]
    pub pipewire: Option<PipeWire>,

    pub compositor_state: CompositorState,
    pub data_control_state: DataControlState,
    pub data_device_state: DataDeviceState,
    pub dmabuf_state: DmabufState,
    pub foreign_toplevel_list_state: ForeignToplevelListState,
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    pub layer_shell_state: WlrLayerShellState,
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
        config_path: Option<std::path::PathBuf>,
    ) -> Self {
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
        let foreign_toplevel_list_state = ForeignToplevelListState::new::<State>(dh);
        let dmabuf_state = DmabufState::new();
        let layer_shell_state = WlrLayerShellState::new::<State>(dh);
        let shm_state =
            ShmState::new::<State>(dh, vec![wl_shm::Format::Xbgr8888, wl_shm::Format::Abgr8888]);
        let session_lock_manager_state = SessionLockManagerState::new::<State, _>(dh, |client| {
            // From: https://wayland.app/protocols/security-context-v1
            // "Compositors should forbid nesting multiple security contexts"
            client
                .get_data::<ClientState>()
                .map_or(true, |data| data.security_context.is_none())
        });
        let xdg_activation_state = XdgActivationState::new::<State>(dh);
        let xdg_shell_state = XdgShellState::new::<State>(dh);
        let xdg_foreign_state = XdgForeignState::new::<State>(dh);
        ContentTypeState::new::<State>(&dh);
        CursorShapeManagerState::new::<State>(&dh);
        TextInputManagerState::new::<State>(&dh);
        InputMethodManagerState::new::<State, _>(&dh, |_| true);
        VirtualKeyboardManagerState::new::<State, _>(&dh, |_| true);
        PointerConstraintsState::new::<State>(&dh);
        TabletManagerState::new::<State>(&dh);
        SecurityContextState::new::<State, _>(&dh, |client| {
            // From: https://wayland.app/protocols/security-context-v1
            // "Compositors should forbid nesting multiple security contexts"
            client
                .get_data::<ClientState>()
                .map_or(true, |data| data.security_context.is_none())
        });
        ScreencopyManagerState::new::<State, _>(&dh, |client| {
            // Same idea as security context state.
            client
                .get_data::<ClientState>()
                .map_or(true, |data| data.security_context.is_none())
        });
        XdgDecorationState::new::<State>(dh);
        FractionalScaleManagerState::new::<State>(dh);
        OutputManagerState::new_with_xdg_output::<State>(dh);
        PresentationState::new::<State>(dh, clock.id() as u32);
        ViewporterState::new::<State>(dh);
        SinglePixelBufferState::new::<State>(dh);
        AlphaModifierState::new::<State>(dh);

        // Initialize a seat and immediatly attach a keyboard and pointer to it.
        // If clients try to connect and do not find any of them they will try to initialize them
        // themselves and chaos will endure.
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(dh, "seat0");

        // Dont let the user crash the compositor with invalid config
        let keyboard_config = &config.input.keyboard;
        let res = seat.add_keyboard(
            keyboard_config.xkb_config(),
            keyboard_config.repeat_delay,
            keyboard_config.repeat_rate,
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
                    keyboard_config.repeat_delay,
                    keyboard_config.repeat_rate,
                )
                .expect("The keyboard is not keyboarding")
            }
        };
        let pointer = seat.add_pointer();
        let cursor_theme_manager = CursorThemeManager::new(config.cursor.clone());
        let keyboard_shortcuts_inhibit_state = KeyboardShortcutsInhibitState::new::<State>(dh);

        let space = Space::new(&config);

        Self {
            display_handle: dh.clone(),
            loop_handle,
            loop_signal,
            stop: false,

            clock,
            suppressed_keys: HashSet::new(),
            seat,
            devices: vec![],
            seat_state,
            keyboard,
            pointer,
            focus_state: FocusState::default(),
            lock_state: LockState::Unlocked,

            dnd_icon: None,
            cursor_theme_manager,
            space,
            unmapped_windows: vec![],
            popups: PopupManager::default(),
            resize_grab_active: false,
            interactive_grab_active: false,
            root_surfaces: HashMap::default(),

            output_state: HashMap::new(),

            config: Arc::new(config),
            cli_config_path: config_path,
            config_ui,
            config_ui_output: None,
            config_watcher,

            #[cfg(feature = "xdg-screencast-portal")]
            pipewire_initialised: std::sync::Once::new(),
            #[cfg(feature = "xdg-screencast-portal")]
            pipewire: None,

            compositor_state,
            data_control_state,
            data_device_state,
            dmabuf_state,
            foreign_toplevel_list_state,
            keyboard_shortcuts_inhibit_state,
            layer_shell_state,
            primary_selection_state,
            shm_state,
            session_lock_manager_state,
            xdg_activation_state,
            xdg_shell_state,
            xdg_foreign_state,
        }
    }

    pub fn add_output(&mut self, output: Output, refresh_interval: Option<Duration>) {
        assert!(
            !self.space.has_output(&output),
            "Tried to add an output twice!"
        );

        info!(name = output.name(), "Adding new output");

        // TODO: Output config and better auto-placement
        let x = self.space.outputs().map(|o| o.geometry().loc.x).sum();
        debug!(?x, y = 0, "Using fallback output location");
        output.change_current_state(None, None, None, Some((x, 0).into()));
        self.space.add_output(output.clone());

        let state = output::OutputState {
            redraw_state: output::RedrawState::Idle,
            frame_clock: FrameClock::new(refresh_interval),
            animations_running: false,
            current_frame_sequence: 0u32,
            pending_screencopies: vec![],
            screencopy_damage_tracker: None,
            debug_damage_tracker: None,
            lock_surface: None,
            // We did not render anything yet, so there's nothing to worry about
            has_lock_backdrop: self.is_locked(),
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
    }

    pub fn remove_output(&mut self, output: &Output) {
        info!(name = output.name(), "Removing output");
        self.space.remove_output(output);

        // Cleanly close [`LayerSurface`] instead of letting them know their demise after noticing
        // the output is gone.
        for layer in layer_map_for_output(output).layers() {
            layer.layer_surface().send_close()
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

        self.queue_redraw(output);
    }

    pub fn output_named(&self, name: &str) -> Option<Output> {
        if name == "active" {
            Some(self.space.active_output().clone())
        } else {
            self.space.outputs().find(|o| &o.name() == name).cloned()
        }
    }

    pub fn queue_redraw(&mut self, output: &Output) {
        let state = self.output_state.get_mut(output).unwrap();
        state.redraw_state.queue();
    }

    pub fn queue_redraw_all(&mut self) {
        let outputs = self.space.outputs().cloned().collect::<Vec<_>>();
        for output in &outputs {
            let state = self.output_state.get_mut(output).unwrap();
            state.redraw_state.queue();
        }
    }

    pub fn send_frames(&self, output: &Output) {
        crate::profile_function!();
        let time = self.clock.now();
        let throttle = Some(Duration::from_secs(1));
        let output_state = self.output_state.get(output).unwrap();
        let sequence = output_state.current_frame_sequence;

        let should_send_frames = |surface: &WlSurface, states: &SurfaceData| {
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
            input_config.keyboard.repeat_rate,
            input_config.keyboard.repeat_delay,
        );

        let disable = per_device_config.map_or(false, |c| c.disable);
        // The device is disabled, no need to apply any configuration
        if disable {
            let _ = device.config_send_events_set_mode(SendEventsMode::DISABLED);
        } else {
            let _ = device.config_send_events_set_mode(SendEventsMode::ENABLED);

            // Aquamarine (hyprland's input backend) determines a libinput device is a mouse by
            // the pointer capability:
            // https://github.com/hyprwm/aquamarine/blob/752d0fbd141fabb5a1e7f865199b80e6e76f8d8e/src/backend/Session.cpp#L826
            //
            // TODO: Separate touchpad config
            if device.has_capability(DeviceCapability::Pointer) {
                let mouse_config = per_device_config.map_or(&input_config.mouse, |c| &c.mouse);

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
                    // TODO: Custom profile when input.rs updates libinput bindings
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

        let Some(idx) = pipewire.casts.iter().position(|c| c.id() == id) else {
            warn!("Tried to stop an invalid cast");
            return;
        };

        let cast = pipewire.casts.swap_remove(idx);
        self.loop_handle.remove(cast.to_compositor_token); // remove calloop stream
        let _ = cast.stream.disconnect(); // even if this fails we dont use the stream anymore

        let object_server = DBUS_CONNECTION.object_server();
        let Ok(interface) = object_server.interface::<_, PortalSession>(&cast.session_handle)
        else {
            warn!(?id, "Cast session doesn't exist");
            return;
        };

        async_io::block_on(async {
            if let Err(err) = interface
                .get()
                .closed(interface.signal_context(), std::collections::HashMap::new())
                .await
            {
                warn!(?err, "Failed to send closed signal to screencast session");
            };
        });
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

#[derive(Default, Debug)]
pub struct FocusState {
    pub keyboard_focus: Option<KeyboardFocusTarget>,
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
    pub border_overrides: BorderOverrides,
    pub open_on_output: Option<String>,
    pub open_on_workspace: Option<usize>,
    pub opacity: Option<f32>,
    pub proportion: Option<f64>,
    pub decoration_mode: Option<DecorationMode>,
    pub maximized: Option<bool>,
    pub fullscreen: Option<bool>,
    pub floating: Option<bool>,
    pub centered: Option<bool>,
    pub centered_in_parent: Option<bool>,
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

        for rule in rules.iter().filter(|rule| {
            rule_matches(
                rule,
                window,
                current_output,
                current_workspace_idx,
                is_focused,
            )
        }) {
            resolved_rules.border_overrides = resolved_rules
                .border_overrides
                .merge_with(rule.border_overrides);
            if let Some(open_on_output) = &rule.open_on_output {
                resolved_rules.open_on_output = Some(open_on_output.clone())
            }

            if let Some(open_on_workspace) = &rule.open_on_workspace {
                resolved_rules.open_on_workspace = Some(open_on_workspace.clone())
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

        false
    }
}
