use std::cell::RefCell;

use smithay::delegate_fractional_scale;
use smithay::desktop::utils::surface_primary_scanout_output;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::{get_parent, with_states, SurfaceData};
use smithay::wayland::fractional_scale::{with_fractional_scale, FractionalScaleHandler};

use crate::state::State;

impl FractionalScaleHandler for State {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        // A surface has asked to get a fractional scale matching its output.
        //
        // First check: The surface has a primary scanout output: then use that output's scale.
        // Second check: The surface is a subsurface, try to use the root's scanout output scale
        // Third check: The surface is root == a toplevel, use the toplevel's workspace output
        // scale.
        #[allow(clippy::redundant_clone)]
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }

        let get_scanout_output = |surface: &WlSurface, states: &SurfaceData| {
            surface_primary_scanout_output(surface, states).or_else(|| {
                // Our custom send frames throlling state.
                let last_callback_output: &RefCell<Option<(Output, u32)>> =
                    states.data_map.get_or_insert(RefCell::default);
                let last_callback_output = last_callback_output.borrow_mut();
                last_callback_output.as_ref().map(|(o, _)| o).cloned()
            })
        };

        let primary_scanout_output = if root != surface {
            // We are the root surface.
            with_states(&root, |states| get_scanout_output(&root, states)).or_else(|| {
                // Use window workspace output.
                self.fht.find_window_and_output(&root).map(|(_, o)| o)
            })
        } else {
            // We are not the root surface, try from surface state.
            with_states(&surface, |states| get_scanout_output(&surface, states)).or_else(|| {
                // Try the root of the surface, if possible
                self.fht.find_window_and_output(&root).map(|(_, o)| o)
            })
        }
        .unwrap_or_else(|| {
            // Final blow: the first available output.
            self.fht.outputs().next().unwrap().clone()
        });

        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional_scale| {
                fractional_scale
                    .set_preferred_scale(primary_scanout_output.current_scale().fractional_scale());
            });
        });
    }
}

delegate_fractional_scale!(State);
