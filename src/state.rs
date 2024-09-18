use std::cell::{RefCell, RefMut};
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use indexmap::IndexMap;
use rustc_hash::FxHashMap;
use smithay::backend::renderer::damage::OutputDamageTracker;
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
use smithay::reexports::calloop::{LoopHandle, LoopSignal, RegistrationToken};
use smithay::reexports::input;
use smithay::reexports::wayland_server::backend::ClientData;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::{Clock, IsAlive, Monotonic};
use smithay::wayland::compositor::{
    with_surface_tree_downward, CompositorClientState, CompositorState, SurfaceData,
    TraversalAction,
};
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::dmabuf::{DmabufFeedback, DmabufState};
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
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::tablet_manager::TabletManagerState;
use smithay::wayland::text_input::TextInputManagerState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;
use smithay::wayland::xdg_activation::XdgActivationState;

use crate::backend::Backend;
use crate::config::{BorderConfig, CONFIG};
use crate::protocols::screencopy::{Screencopy, ScreencopyManagerState};
use crate::shell::cursor::CursorThemeManager;
use crate::shell::workspaces::tile::Tile;
use crate::shell::workspaces::{WorkspaceId, WorkspaceSet};
use crate::shell::KeyboardFocusTarget;
use crate::utils::output::OutputExt;
#[cfg(feature = "xdg-screencast-portal")]
use crate::utils::pipewire::PipeWire;
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
        _socket_name: String,
    ) -> Self {
        let mut fht = Fht::new(dh, loop_handle, loop_signal);
        let backend: crate::backend::Backend = if let Ok(backend_name) =
            std::env::var("FHTC_BACKEND")
        {
            match backend_name.trim().to_lowercase().as_str() {
                #[cfg(feature = "x11_backend")]
                "x11" => crate::backend::x11::X11Data::new(&mut fht).unwrap().into(),
                #[cfg(feature = "udev_backend")]
                "kms" | "udev" => crate::backend::udev::UdevData::new(&mut fht)
                    .unwrap()
                    .into(),
                x => unimplemented!("No such backend implemented!: {x}"),
            }
        } else if std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok() {
            info!("Detected (WAYLAND_)DISPLAY. Running in nested X11 window");
            #[cfg(feature = "x11_backend")]
            {
                crate::backend::x11::X11Data::new(&mut fht).unwrap().into()
            }
            #[cfg(not(feature = "x11_backend"))]
            panic!("X11 backend not enabled on this build! Enable the 'x11_backend' feature when building")
        } else {
            info!("Running from TTY, initializing Udev backend");
            #[cfg(feature = "udev_backend")]
            {
                crate::backend::udev::UdevData::new(&mut fht)
                    .unwrap()
                    .into()
            }
            #[cfg(not(feature = "udev_backend"))]
            panic!("Udev backend not enabled on this build! Enable the 'udev_backend' feature when building")
        };

        Self { fht, backend }
    }

    #[profiling::function]
    pub fn dispatch(&mut self) -> anyhow::Result<()> {
        self.fht
            .workspaces_mut()
            .for_each(|(_, wset)| wset.refresh());
        self.fht.popups.cleanup();
        // Redraw queued outputs.
        {
            profiling::scope!("redraw_queued_outputs");
            for output in self
                .fht
                .outputs()
                .filter_map(|o| {
                    let is_queued = OutputState::get(o).render_state.is_queued();
                    is_queued.then(|| o.clone())
                })
                .collect::<Vec<_>>()
            {
                self.redraw(output);
            }
        }

        // Make sure the surface is not dead (otherwise wayland wont be happy)
        // NOTE: focus_target from state is always guaranteed to be the same as keyboard focus.
        let old_focus_dead = self
            .fht
            .focus_state
            .focus_target
            .as_ref()
            .is_some_and(|ft| !ft.alive());
        {
            profiling::scope!("refresh_focus");
            if old_focus_dead {
                // We are focusing nothing, default to the active workspace focused window.
                if let Some(window) = self.fht.focus_state.output.as_ref().and_then(|o| {
                    let active = self.fht.wset_for(o).active();
                    active.focused()
                }) {
                    self.set_focus_target(Some(window.into()));
                } else {
                    // just reset
                    self.set_focus_target(None);
                }
            }
        }

        {
            profiling::scope!("DislpayHandle::flush_clients");
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

    #[profiling::function]
    pub fn redraw(&mut self, output: Output) {
        // Verify our invariant.
        let mut output_state = OutputState::get(&output);
        assert!(output_state.render_state.is_queued());

        // Advance animations.
        let current_time = self.fht.clock.now();
        output_state.animations_running = self.fht.advance_animations(&output, current_time);
        drop(output_state);

        // Then ask the backend to render.
        // if res.is_err() == something wrong happened and we didnt render anything.
        // if res == Ok(true) we rendered and submitted a new buffer
        // if res == Ok(false) we rendered but had no damage to submit
        let res = self.backend.render(&mut self.fht, &output, current_time);

        {
            let mut output_state = OutputState::get(&output);

            if res.is_err() {
                // Update the redraw state on failed render.
                output_state.render_state =
                    if let RenderState::WaitingForVblankTimer { token, .. } =
                        output_state.render_state
                    {
                        RenderState::WaitingForVblankTimer {
                            token,
                            queued: false,
                        }
                    } else {
                        RenderState::Idle
                    };
            }
        }

        // Send frame callbacks
        self.fht.send_frames(&output);
    }
}

pub struct Fht {
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, State>,
    pub loop_signal: LoopSignal,
    pub stop: Arc<AtomicBool>,

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
    pub workspaces: IndexMap<Output, WorkspaceSet>,
    pub unmapped_windows: Vec<UnmappedWindow>,
    pub focus_state: FocusState,
    pub popups: PopupManager,
    pub root_surfaces: FxHashMap<WlSurface, WlSurface>,

    pub last_config_error: Option<anyhow::Error>,

    #[cfg(feature = "xdg-screencast-portal")]
    pub pipewire_initialised: std::sync::Once,
    #[cfg(feature = "xdg-screencast-portal")]
    pub pipewire: Option<PipeWire>,

    pub compositor_state: CompositorState,
    pub data_control_state: DataControlState,
    pub data_device_state: DataDeviceState,
    pub dmabuf_state: DmabufState,
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    pub layer_shell_state: WlrLayerShellState,
    pub primary_selection_state: PrimarySelectionState,
    pub shm_state: ShmState,
    pub xdg_activation_state: XdgActivationState,
    pub xdg_shell_state: XdgShellState,
}

impl Fht {
    pub fn new(
        dh: &DisplayHandle,
        loop_handle: LoopHandle<'static, State>,
        loop_signal: LoopSignal,
    ) -> Self {
        let clock = Clock::<Monotonic>::new();

        let compositor_state = CompositorState::new_v6::<State>(dh);
        let primary_selection_state = PrimarySelectionState::new::<State>(dh);
        let data_control_state =
            DataControlState::new::<State, _>(dh, Some(&primary_selection_state), |_| true);
        let data_device_state = DataDeviceState::new::<State>(dh);
        let dmabuf_state = DmabufState::new();
        let layer_shell_state = WlrLayerShellState::new::<State>(dh);
        let shm_state =
            ShmState::new::<State>(dh, vec![wl_shm::Format::Xbgr8888, wl_shm::Format::Abgr8888]);
        let xdg_activation_state = XdgActivationState::new::<State>(dh);
        let xdg_shell_state = XdgShellState::new::<State>(dh);
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

        // Initialize a seat and immediatly attach a keyboard and pointer to it.
        // If clients try to connect and do not find any of them they will try to initialize them
        // themselves and chaos will endure.
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(dh, "seat0");

        // Dont let the user crash the compositor with invalid config
        let keyboard_config = &CONFIG.input.keyboard;
        let res = seat.add_keyboard(
            keyboard_config.get_xkb_config(),
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
        let cursor_theme_manager = CursorThemeManager::new();
        let keyboard_shortcuts_inhibit_state = KeyboardShortcutsInhibitState::new::<State>(dh);

        Self {
            display_handle: dh.clone(),
            loop_handle,
            loop_signal,
            stop: Arc::new(AtomicBool::new(false)),

            clock,
            suppressed_keys: HashSet::new(),
            seat,
            devices: vec![],
            seat_state,
            keyboard,
            pointer,
            focus_state: FocusState::default(),

            dnd_icon: None,
            cursor_theme_manager,
            workspaces: IndexMap::new(),
            unmapped_windows: vec![],
            popups: PopupManager::default(),
            resize_grab_active: false,
            interactive_grab_active: false,
            root_surfaces: FxHashMap::default(),

            last_config_error: None,

            #[cfg(feature = "xdg-screencast-portal")]
            pipewire_initialised: std::sync::Once::new(),
            #[cfg(feature = "xdg-screencast-portal")]
            pipewire: None,

            compositor_state,
            data_control_state,
            data_device_state,
            dmabuf_state,
            keyboard_shortcuts_inhibit_state,
            layer_shell_state,
            primary_selection_state,
            shm_state,
            xdg_activation_state,
            xdg_shell_state,
        }
    }
}

impl Fht {
    pub fn outputs(&self) -> impl Iterator<Item = &Output> {
        self.workspaces.keys()
    }

    pub fn add_output(&mut self, output: Output) {
        assert!(
            self.workspaces.get(&output).is_none(),
            "Tried to add an output twice!"
        );

        info!(name = output.name(), "Adding new output");

        // Current default behaviour:
        //
        // When adding an output, put it to the right of every other output.
        // Right now this assumption can be false for alot of users, but this is just as a
        // fallback.
        let x: i32 = self.outputs().map(|o| o.geometry().loc.x).sum();
        debug!(?x, y = 0, "Using fallback output location");
        output.change_current_state(None, None, None, Some((x, 0).into()));

        let workspace_set = WorkspaceSet::new(output.clone());
        self.workspaces.insert(output.clone(), workspace_set);

        // Focus output now.
        if CONFIG.general.cursor_warps {
            let center = output.geometry().center();
            self.loop_handle.insert_idle(move |state| {
                state.move_pointer(center.to_f64());
            });
        }
        self.focus_state.output = Some(output);
    }

    pub fn remove_output(&mut self, output: &Output) {
        info!(name = output.name(), "Removing output");
        let mut removed_wset = self
            .workspaces
            .swap_remove(output)
            .expect("Tried to remove a non-existing output!");

        if self.workspaces.is_empty() {
            // There's nothing more todo, just adandon everything.
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            return;
        }

        let wset = self.workspaces.first_mut().unwrap().1;
        wset.merge_with(removed_wset);

        // Cleanly close [`LayerSurface`] instead of letting them know their demise after noticing
        // the output is gone.
        for layer in layer_map_for_output(output).layers() {
            layer.layer_surface().send_close()
        }

        wset.refresh();
        wset.arrange();
    }

    pub fn output_resized(&mut self, output: &Output) {
        layer_map_for_output(output).arrange();
        self.wset_mut_for(output).output_resized();
    }

    pub fn active_output(&self) -> Output {
        self.focus_state
            .output
            .clone()
            .unwrap_or_else(|| self.outputs().next().unwrap().clone())
    }

    pub fn output_named(&self, name: &str) -> Option<Output> {
        if name == "active" {
            Some(self.active_output())
        } else {
            self.outputs().find(|o| &o.name() == name).cloned()
        }
    }

    pub fn workspaces(&self) -> impl Iterator<Item = (&Output, &WorkspaceSet)> {
        self.workspaces.iter()
    }

    pub fn workspaces_mut(&mut self) -> impl Iterator<Item = (&Output, &mut WorkspaceSet)> {
        self.workspaces.iter_mut()
    }

    pub fn wset_for(&self, output: &Output) -> &WorkspaceSet {
        self.workspaces
            .get(output)
            .expect("Tried to get the WorkspaceSet of a non-existing output!")
    }

    pub fn wset_mut_for(&mut self, output: &Output) -> &mut WorkspaceSet {
        self.workspaces
            .get_mut(output)
            .expect("Tried to get the WorkspaceSet of a non-existing output!")
    }
}

impl Fht {
    #[profiling::function]
    pub fn send_frames(&self, output: &Output) {
        let time = self.clock.now();
        let throttle = Some(Duration::from_secs(1));
        let sequence = OutputState::get(output).current_frame_sequence;

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

        if let CursorImageStatus::Surface(surface) = self.cursor_theme_manager.image_status() {
            send_frames_surface_tree(surface, output, time, throttle, should_send_frames);
        }

        if let Some(surface) = &self.dnd_icon {
            send_frames_surface_tree(surface, output, time, throttle, should_send_frames);
        }

        for window in self.wset_for(output).visible_windows() {
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

        for window in self.wset_for(output).visible_windows() {
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

        for window in self.wset_for(output).visible_windows() {
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

    #[profiling::function]
    pub fn take_presentation_feedback(
        &self,
        output: &Output,
        render_element_states: &RenderElementStates,
    ) -> OutputPresentationFeedback {
        let mut output_presentation_feedback = OutputPresentationFeedback::new(output);

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

        for window in self.wset_for(output).visible_windows() {
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
    pub output: Option<Output>,
    pub focus_target: Option<KeyboardFocusTarget>,
}

#[derive(Debug)]
pub struct OutputState {
    pub render_state: RenderState,

    pub animations_running: bool,

    pub current_frame_sequence: u32,

    pub pending_screencopy: Option<Screencopy>,

    pub damage_tracker: OutputDamageTracker,
}

impl OutputState {
    pub fn get(output: &Output) -> RefMut<'_, Self> {
        output.user_data().insert_if_missing(|| Self::new(output));
        output
            .user_data()
            .get::<RefCell<Self>>()
            .unwrap()
            .borrow_mut()
    }

    pub fn new(output: &Output) {
        output.user_data().insert_if_missing(|| {
            RefCell::new(Self {
                render_state: RenderState::Idle,
                animations_running: false,
                current_frame_sequence: 0,
                pending_screencopy: None,
                damage_tracker: OutputDamageTracker::from_output(output),
            })
        });
    }
}

#[derive(Debug, Default)]
pub enum RenderState {
    #[default]
    Idle,
    Queued,
    WaitingForVblank {
        redraw_needed: bool,
    },
    WaitingForVblankTimer {
        token: RegistrationToken,
        queued: bool,
    },
}

impl RenderState {
    #[inline(always)]
    pub fn is_queued(&self) -> bool {
        matches!(
            self,
            RenderState::Queued | RenderState::WaitingForVblankTimer { queued: true, .. }
        )
    }

    pub fn queue(&mut self) {
        *self = match std::mem::take(self) {
            Self::Idle => Self::Queued,
            Self::WaitingForVblank {
                redraw_needed: false,
            } => Self::WaitingForVblank {
                redraw_needed: true,
            },
            Self::WaitingForVblankTimer {
                token,
                queued: false,
            } => Self::WaitingForVblankTimer {
                token,
                queued: true,
            },
            // We are already queued
            value => value,
        }
    }
}

// We track ourselves window configure state since some clients may set initial_configure_sent to
// true even if its NOT (example: electron + ozone wayland)
pub enum UnmappedWindow {
    Unconfigured(Window),
    Configured {
        window: Window,
        border_config: Option<BorderConfig>,
        /// The workspace to open the window on
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
