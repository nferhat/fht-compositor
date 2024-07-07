use smithay::delegate_pointer_constraints;
use smithay::input::pointer::PointerHandle;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraintsHandler};
use smithay::wayland::seat::WaylandFocus;

use crate::state::State;

impl PointerConstraintsHandler for State {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        if pointer
            .current_focus()
            .and_then(|x| x.wl_surface().map(|s| s.into_owned()))
            .is_some_and(|s| s == *surface)
        {
            return;
        }

        with_pointer_constraint(surface, pointer, |constraint| {
            constraint.unwrap().activate();
        })
    }
}

delegate_pointer_constraints!(State);
