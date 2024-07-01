use smithay::delegate_xdg_shell;
use smithay::desktop::{
    find_popup_root_surface, layer_map_for_output, PopupKeyboardGrab, PopupKind, PopupPointerGrab,
    PopupUngrabStrategy, Window, WindowSurfaceType,
};
use smithay::input::pointer::Focus;
use smithay::input::Seat;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::WmCapabilities;
use smithay::reexports::wayland_server::protocol::{wl_output, wl_seat};
use smithay::utils::Serial;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};

use crate::shell::workspaces::tile::WorkspaceElement;
use crate::shell::KeyboardFocusTarget;
use crate::state::State;

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.fht.xdg_shell_state
    }

    fn new_toplevel(&mut self, toplevel: ToplevelSurface) {
        let window = Window::new_wayland_window(toplevel);
        self.fht.pending_windows.push(window.into());
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.fht.unconstrain_popup(&surface);
        if let Err(err) = self.fht.popups.track_popup(PopupKind::from(surface)) {
            warn!(?err, "Failed to track popup!")
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, _: wl_seat::WlSeat, serial: Serial) {
        if let Some(window) = self.fht.find_window(surface.wl_surface()).cloned() {
            self.handle_move_request(window, serial);
        }
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat: Seat<State> = Seat::from_resource(&seat).unwrap();
        let popup_kind = PopupKind::Xdg(surface);

        if let Some(root) = find_popup_root_surface(&popup_kind).ok().and_then(|root| {
            self.fht
                .find_window(&root)
                .cloned()
                .map(KeyboardFocusTarget::Window)
                .or_else(|| {
                    self.fht
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
        if let Some((window, ws)) = self
            .fht
            .find_window_and_workspace_mut(toplevel.wl_surface())
        {
            window.set_maximized(true);
            ws.arrange_tiles();
        }

        toplevel.send_configure();
    }

    fn unmaximize_request(&mut self, toplevel: ToplevelSurface) {
        if let Some((window, ws)) = self
            .fht
            .find_window_and_workspace_mut(toplevel.wl_surface())
        {
            window.set_maximized(false);
            ws.arrange_tiles();
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
            let requested_output = wl_output.as_ref().and_then(Output::from_resource);

            // If the surface request for a specific output to be fullscreened on, move it to the
            // active workspace of that output, then fullscreen it.
            //
            // If not, then fullscreen it inside its active workspace.
            if let Some((window, requested_output, mut output)) =
                requested_output.and_then(|requested_output| {
                    let (window, current_output) = self.fht.find_window_and_output(wl_surface)?;
                    Some((window.clone(), requested_output, current_output.clone()))
                })
            {
                if requested_output != output {
                    output = requested_output;

                    let ws = self.fht.ws_mut_for(&window).unwrap();
                    let tile = ws.remove_tile(&window).unwrap();

                    let new_ws = self.fht.wset_mut_for(&output).active_mut();
                    new_ws.insert_tile(tile);
                }

                let ws = self.fht.ws_mut_for(&window).unwrap();
                ws.fullscreen_element(&window);
            } else if let Some(window) = self.fht.find_window(wl_surface).cloned() {
                let ws = self.fht.ws_mut_for(&window).unwrap();
                ws.fullscreen_element(&window);
            }
        }

        surface.send_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.fht.find_window(surface.wl_surface()) {
            window.set_fullscreen(false);
        }

        surface.send_configure();
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

delegate_xdg_shell!(State);
