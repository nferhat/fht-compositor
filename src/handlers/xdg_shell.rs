use smithay::delegate_xdg_shell;
use smithay::desktop::{
    find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output, PopupKeyboardGrab,
    PopupKind, PopupPointerGrab, PopupUngrabStrategy, WindowSurfaceType,
};
use smithay::input::pointer::Focus;
use smithay::input::Seat;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::{
    self, WmCapabilities,
};
use smithay::reexports::wayland_server::protocol::{wl_output, wl_seat};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::Serial;
use smithay::wayland::compositor::{
    add_pre_commit_hook, with_states, BufferAssignment, SurfaceAttributes,
};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};

use crate::focus_target::KeyboardFocusTarget;
use crate::input::resize_tile_grab::{ResizeEdge, ResizeTileGrab};
use crate::input::swap_tile_grab::SwapTileGrab;
use crate::output::OutputExt;
use crate::state::{Fht, State, UnmappedWindow};
use crate::window::Window;

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.fht.xdg_shell_state
    }

    fn new_toplevel(&mut self, toplevel: ToplevelSurface) {
        let window = Window::new(toplevel);
        self.fht
            .unmapped_windows
            .push(UnmappedWindow::Unconfigured(window));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(idx) = self.fht.unmapped_windows.iter().position(|unmapped| {
            unmapped
                .window()
                .wl_surface()
                .is_some_and(|s| &*s == surface.wl_surface())
        }) {
            let _unmapped_tile = self.fht.unmapped_windows.remove(idx);
            return;
        }

        let Some((window, workspace)) = self
            .fht
            .space
            .find_window_and_workspace_mut(surface.wl_surface())
        else {
            warn!("Destroyed toplevel missing from mapped windows and unmapped windows");
            return;
        };

        self.backend.with_renderer(|renderer| {
            if workspace.prepare_close_animation_for_window(&window, renderer) {
                workspace.close_window(&window, renderer, true);
            }
        });

        let output = workspace.output().clone();
        self.fht.queue_redraw(&output);

        // dont forget to remove the foreign toplevel handle.
        //
        // NOTE: I am not sure but this should always be emitted, regardless of whether we or the
        // toplevel closes (since we use send_close request)
        self.fht.close_foreign_handle(&window);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.fht.unconstrain_popup(&surface);
        if let Err(err) = self.fht.popups.track_popup(PopupKind::from(surface)) {
            warn!(?err, "Failed to track popup!")
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, _: wl_seat::WlSeat, serial: Serial) {
        let pointer = self.fht.pointer.clone();
        let mut grab_start_data = None;

        pointer.with_grab(|grab_serial, grab| {
            if grab_serial == serial {
                let start_data = grab.start_data();
                if start_data
                    .focus
                    .as_ref()
                    .is_some_and(|(focus, _)| focus.same_client_as(&surface.wl_surface().id()))
                {
                    grab_start_data = Some(start_data.clone());
                }
            }
        });

        let Some(start_data) = grab_start_data else {
            return;
        };

        let mut output = None;
        if let Some((window, workspace)) = self
            .fht
            .space
            .find_window_and_workspace_mut(surface.wl_surface())
        {
            if workspace.start_interactive_swap(&window) {
                let ws_output = workspace.output().clone();
                output = Some(ws_output.clone()); // augh, the borrow checker
                let grab = SwapTileGrab {
                    window,
                    output: ws_output,
                    start_data,
                };
                pointer.set_grab(self, grab, serial, Focus::Clear);
            }
        }

        if let Some(ref output) = output {
            self.fht.queue_redraw(output);
        }
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let pointer = self.fht.pointer.clone();
        let mut grab_start_data = None;

        pointer.with_grab(|grab_serial, grab| {
            if grab_serial == serial {
                let start_data = grab.start_data();
                if start_data
                    .focus
                    .as_ref()
                    .is_some_and(|(focus, _)| focus.same_client_as(&surface.wl_surface().id()))
                {
                    grab_start_data = Some(start_data.clone());
                }
            }
        });

        let Some(start_data) = grab_start_data else {
            return;
        };

        let mut output = None;
        if let Some((window, workspace)) = self
            .fht
            .space
            .find_window_and_workspace_mut(surface.wl_surface())
        {
            let edges = ResizeEdge::from(edges);
            if workspace.start_interactive_resize(&window, edges) {
                let ws_output = workspace.output().clone();
                output = Some(ws_output.clone()); // augh, the borrow checker
                let grab = ResizeTileGrab {
                    window,
                    output: ws_output,
                    start_data,
                };
                pointer.set_grab(self, grab, serial, Focus::Clear);
            }
        }

        if let Some(ref output) = output {
            self.fht.queue_redraw(output);
        }
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat: Seat<State> = Seat::from_resource(&seat).unwrap();
        let popup_kind = PopupKind::Xdg(surface);

        if let Some(root) = find_popup_root_surface(&popup_kind).ok().and_then(|root| {
            self.fht
                .space
                .find_window(&root)
                .map(KeyboardFocusTarget::Window)
                .or_else(|| {
                    self.fht
                        .space
                        .outputs()
                        .find_map(|o| {
                            layer_map_for_output(o)
                                .layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)
                                .cloned()
                        })
                        .map(KeyboardFocusTarget::LayerSurface)
                })
        }) {
            let grab = self.fht.popups.grab_popup(root, popup_kind, &seat, serial);

            if let Ok(mut grab) = grab {
                if let Some(keyboard) = seat.get_keyboard() {
                    if keyboard.is_grabbed()
                        && !(keyboard.has_grab(serial)
                            || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
                    {
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }
                    keyboard.set_focus(self, grab.current_grab(), serial);
                    keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
                }
                if let Some(pointer) = seat.get_pointer() {
                    if pointer.is_grabbed()
                        && !(pointer.has_grab(serial)
                            || pointer
                                .has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
                    {
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }
                    pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
                }
            }
        }
    }

    fn maximize_request(&mut self, toplevel: ToplevelSurface) {
        if toplevel
            .current_state()
            .capabilities
            .contains(WmCapabilities::Maximize)
        {
            let wl_surface = toplevel.wl_surface();
            if let Some(window) = self.fht.space.find_window(wl_surface) {
                if self.fht.space.maximize_window(
                    &window,
                    true,
                    !self.fht.config.animations.disable,
                ) {
                    window.request_maximized(true);
                }
            }
        }

        toplevel.send_configure();
    }

    fn unmaximize_request(&mut self, toplevel: ToplevelSurface) {
        if let Some((window, ws)) = self
            .fht
            .space
            .find_window_and_workspace_mut(toplevel.wl_surface())
        {
            window.request_maximized(false);
            ws.arrange_tiles(true);
        }

        toplevel.send_configure();
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        wl_output: Option<wl_output::WlOutput>,
    ) {
        if surface
            .current_state()
            .capabilities
            .contains(WmCapabilities::Fullscreen)
        {
            let wl_surface = surface.wl_surface();
            if let Some(window) = self.fht.space.find_window(wl_surface) {
                if let Some(requested) = wl_output.as_ref().and_then(Output::from_resource) {
                    self.fht
                        .space
                        .move_window_to_output(&window, &requested, true);
                }

                window.request_fullscreen(true);
                if !self.fht.space.fullscreen_window(&window, true) {
                    window.request_fullscreen(false);
                }
            }
        }

        surface.send_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.fht.space.find_window(surface.wl_surface()) {
            // NOTE: Workspaces take care of unfullscreening and arranging
            window.request_fullscreen(false);
        }

        surface.send_configure();
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.fht.space.find_window(surface.wl_surface()) {
            self.fht.send_foreign_window_details(&window);
            self.fht.resolve_rules_for_window(&window);
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.fht.space.find_window(surface.wl_surface()) {
            self.fht.send_foreign_window_details(&window);
            self.fht.resolve_rules_for_window(&window);
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        self.fht.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }
}

pub(super) fn add_window_pre_commit_hook(window: &Window) {
    // The workspace tile api is not responsible for actually starting the close animations, we are
    // the ones that should do this.
    let wl_surface = window.wl_surface().unwrap();
    let hook_id = add_pre_commit_hook::<State, _>(&wl_surface, |state, _dh, surface| {
        let Some((window, workspace)) = state.fht.space.find_window_and_workspace_mut(surface)
        else {
            warn!("Window pre-commit hook should be removed when unmapped");
            return;
        };

        // Before commiting, we check if the window's buffers are getting unmapped.
        // If that's the case, the window is likely closing (or minimizing, if the
        // compositor supports that)
        //
        // Since we are going to close, we take a snapshot of the window's elements,
        // like we do inside `Tile::render_elements` into a
        // GlesTexture and store that for future use.
        let got_unmapped = with_states(surface, |states| {
            let mut guard = states.cached_state.get::<SurfaceAttributes>();
            let attrs = guard.pending();
            matches!(attrs.buffer, Some(BufferAssignment::Removed))
        });

        if got_unmapped {
            state.backend.with_renderer(|renderer| {
                workspace.prepare_close_animation_for_window(&window, renderer);
            });
        } else {
            workspace.clear_close_animation_for_window(&window);
        }
    });

    window.set_pre_commit_hook_id(hook_id);
}

impl Fht {
    pub fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some((window, workspace)) = self.space.find_window_and_workspace(&root) else {
            return;
        };

        // The target (aka the popup) geometry should be relative to the parent (aka the window's)
        // geometry, based on the xdg_shell protocol requirements.
        let window_loc = workspace.window_location(&window).unwrap();
        let mut target = workspace.output().geometry();
        target.loc = window_loc + get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

delegate_xdg_shell!(State);
