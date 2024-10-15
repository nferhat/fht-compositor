use smithay::delegate_pointer_constraints;
use smithay::input::pointer::PointerHandle;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};
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

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        // Implementation copied from anvil
        if with_pointer_constraint(surface, pointer, |constraint| {
            constraint.map_or(false, |c| c.is_active())
        }) {
            let origin = self
                .fht
                .space
                .find_window(surface)
                .map(|w| w.render_offset())
                .unwrap_or_default()
                .to_f64();

            pointer.set_location(origin + location);
        }
    }
}

delegate_pointer_constraints!(State);
