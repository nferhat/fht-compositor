use smithay::delegate_xdg_shell;
use smithay::desktop::{
    find_popup_root_surface, layer_map_for_output, PopupKeyboardGrab, PopupKind, PopupPointerGrab,
    PopupUngrabStrategy, WindowSurfaceType,
};
use smithay::input::pointer::{CursorIcon, CursorImageStatus, Focus};
use smithay::input::Seat;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::{
    self, WmCapabilities,
};
use smithay::reexports::wayland_server::protocol::{wl_output, wl_seat};
use smithay::utils::Serial;
use smithay::wayland::compositor::{
    add_pre_commit_hook, with_states, BufferAssignment, SurfaceAttributes,
};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};

use crate::input::swap_tile_grab::SwapTileGrab;
use crate::shell::KeyboardFocusTarget;
use crate::state::{OutputState, State, UnmappedWindow};
use crate::window::Window;

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.fht.xdg_shell_state
    }

    fn new_toplevel(&mut self, toplevel: ToplevelSurface) {
        let window = Window::new(toplevel);
        add_window_pre_commit_hook(&window);
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
        OutputState::get(&workspace.output()).render_state.queue();

        self.backend.with_renderer(|renderer| {
            if workspace.prepare_close_animation_for_window(&window, renderer) {
                workspace.close_window(&window, renderer, true);
            }
        });
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.fht.unconstrain_popup(&surface);
        if let Err(err) = self.fht.popups.track_popup(PopupKind::from(surface)) {
            warn!(?err, "Failed to track popup!")
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, _: wl_seat::WlSeat, serial: Serial) {
        let pointer = self.fht.pointer.clone();
        if let Some(window) = self.fht.space.find_window(surface.wl_surface()) {
            // TODO: Handle grabs
            if !pointer.has_grab(serial) {
                return;
            }
            let Some(start_data) = pointer.grab_start_data() else {
                return;
            };
            if self.fht.space.start_interactive_swap(&window) {
                self.fht.loop_handle.insert_idle(|state| {
                    // TODO: Figure out why I have todo this inside a idle
                    state.fht.interactive_grab_active = true;
                    state
                        .fht
                        .cursor_theme_manager
                        .set_image_status(CursorImageStatus::Named(CursorIcon::Grabbing));
                });
                let grab = SwapTileGrab { window, start_data };
                pointer.set_grab(self, grab, serial, Focus::Clear);
            }
        }
    }

    fn resize_request(
        &mut self,
        _: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _: Serial,
        _: xdg_toplevel::ResizeEdge,
    ) {
        // TODO: Handle resize requests
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
        if let Some((window, workspace)) = self
            .fht
            .space
            .find_window_and_workspace_mut(toplevel.wl_surface())
        {
            window.request_maximized(true);
            workspace.arrange_tiles(true);
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

                self.fht.space.fullscreen_window(&window, true);
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
            self.fht.resolve_rules_for_window(&window);
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.fht.space.find_window(surface.wl_surface()) {
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

fn add_window_pre_commit_hook(window: &Window) {
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

delegate_xdg_shell!(State);
