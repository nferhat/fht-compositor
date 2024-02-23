use smithay::wayland::idle_inhibit::IdleInhibitHandler;

use crate::state::State;

impl IdleInhibitHandler for State {
    fn inhibit(
        &mut self,
        _surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        todo!()
    }

    fn uninhibit(
        &mut self,
        _surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        todo!()
    }
}
