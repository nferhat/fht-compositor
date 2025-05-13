use std::borrow::Cow;

use smithay::delegate_pointer_constraints;
use smithay::input::pointer::PointerHandle;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};
use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraintsHandler};
use smithay::wayland::seat::WaylandFocus;

use crate::state::{Fht, State};

impl Fht {
    /// Activate the pointer constraint associated with the currently focused surface, if any.
    pub fn activate_pointer_constraint(&mut self) {
        let pointer = self.seat.get_pointer().unwrap();
        let pointer_loc = pointer.current_location();

        let Some((under, surface_loc)) = self.focus_target_under(pointer_loc) else {
            return;
        };
        let Some(surface) = under.wl_surface() else {
            return;
        };

        with_pointer_constraint(&surface, &pointer, |constraint| {
            let Some(constraint) = constraint else { return };

            if constraint.is_active() {
                return;
            }

            // Constraint does not apply if not within region.
            if let Some(region) = constraint.region() {
                let pos_within_surface = pointer_loc - surface_loc.to_f64();
                if !region.contains(pos_within_surface.to_i32_round()) {
                    return;
                }
            }

            constraint.activate();
        });
    }
}

impl PointerConstraintsHandler for State {
    fn new_constraint(&mut self, _: &WlSurface, _: &PointerHandle<Self>) {
        self.update_pointer_focus();
        self.fht.activate_pointer_constraint();
    }

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        // Implementation copied from anvil
        if with_pointer_constraint(surface, pointer, |constraint| {
            constraint.is_some_and(|c| c.is_active())
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

            let Some(window) = self.fht.space.find_window(surface) else {
                return;
            };
            let window_loc = self.fht.space.window_location(&window).unwrap();
            pointer.set_location(window_loc.to_f64() + window.render_offset().to_f64() + location);
        }
    }
}

delegate_pointer_constraints!(State);
