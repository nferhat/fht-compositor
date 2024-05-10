use smithay::delegate_xdg_shell;
use smithay::desktop::{
    find_popup_root_surface, layer_map_for_output, PopupKeyboardGrab, PopupKind, PopupPointerGrab,
    PopupUngrabStrategy, Window, WindowSurfaceType,
};
use smithay::input::pointer::Focus;
use smithay::input::Seat;
use smithay::reexports::wayland_server::protocol::wl_seat;
use smithay::utils::Serial;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};

use crate::shell::KeyboardFocusTarget;
use crate::state::State;

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.fht.xdg_shell_state
    }

    fn new_toplevel(&mut self, toplevel: ToplevelSurface) {
        let window = Window::new_wayland_window(toplevel);
        self.fht.pending_windows.push(window);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.fht.unconstrain_popup(&surface);
        if let Err(err) = self.fht.popups.track_popup(PopupKind::from(surface)) {
            warn!(?err, "Failed to track popup!")
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, _: wl_seat::WlSeat, serial: Serial) {
        // TODO:
        // if let Some(window) = self.fht.find_window(surface.wl_surface()).cloned() {
        //     self.handle_move_request(window, serial);
        // }
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

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        // TODO: Make xdg_shell request work.
        surface.send_configure();
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        // TODO: Make xdg_shell request work.
        surface.send_configure();
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        mut wl_output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        // TODO: Make xdg_shell request work.
        surface.send_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        // TODO: Make xdg_shell request work.
        surface.send_configure();
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        // TODO: Make xdg_shell request work.
        surface.send_configure();
    }
}

delegate_xdg_shell!(State);
