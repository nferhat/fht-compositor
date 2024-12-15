use std::borrow::Cow;

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
            let Some(focused_surface) = pointer
                .current_focus()
                .and_then(|f| f.wl_surface().map(Cow::into_owned))
            else {
                return;
            };

            if &focused_surface != surface {
                // only focused surfaec can give position hint
                // this is to avoid random cursor warps around the screen
                return;
            }

            // TODO: cursor_position_hint for layer surfaces?
            let Some(window) = self.fht.space.find_window(surface) else {
                return;
            };
            let window_loc = self.fht.space.window_location(&window).unwrap();
            pointer.set_location(window_loc.to_f64() + window.render_offset().to_f64() + location);
        }
    }
}

delegate_pointer_constraints!(State);
