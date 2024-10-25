pub mod cursor;
pub mod focus_target;

use smithay::desktop::utils::under_from_surface_tree;
use smithay::desktop::{
    find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output, LayerSurface,
    PopupKind, WindowSurfaceType,
};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::Layer;
use smithay::wayland::shell::xdg::PopupSurface;

pub use self::focus_target::{KeyboardFocusTarget, PointerFocusTarget};
use crate::state::{Fht, OutputState};
use crate::utils::output::OutputExt;

impl Fht {
    pub fn focus_target_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(PointerFocusTarget, Point<f64, Logical>)> {
        let output = self.space.active_output();
        let output_loc = output.current_location();
        let point_in_output = point - output_loc.to_f64();
        let layer_map = layer_map_for_output(output);

        {
            let output_state = OutputState::get(output);
            if let Some(lock_surface) = &output_state.lock_surface {
                // NOTE: Lock surface is always position at (0,0)
                if let Some((surface, surface_loc)) = under_from_surface_tree(
                    lock_surface.wl_surface(),
                    point_in_output,
                    Point::default(),
                    WindowSurfaceType::ALL,
                ) {
                    return Some((
                        PointerFocusTarget::WlSurface(surface),
                        (surface_loc + output_loc).to_f64(),
                    ));
                }
            }
        }

        if let Some(layer) = layer_map.layer_under(Layer::Overlay, point_in_output) {
            let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
            if let Some((surface, surface_loc)) =
                layer.surface_under(point_in_output - layer_loc.to_f64(), WindowSurfaceType::ALL)
            {
                return Some((
                    PointerFocusTarget::WlSurface(surface),
                    (surface_loc + output_loc + layer_loc).to_f64(),
                ));
            }
        }

        if let Some((fullscreen, mut fullscreen_loc)) = self.space.fullscreened_window(point) {
            fullscreen_loc -= fullscreen.render_offset();
            let window_wl_surface = fullscreen.wl_surface().unwrap();
            // NOTE: window location passed here is already global, since its from
            // `Fht::window_geometry`
            if let Some(ret) = fullscreen
                .surface_under(
                    point_in_output - fullscreen_loc.to_f64(),
                    WindowSurfaceType::ALL,
                )
                .map(|(surface, surface_loc)| {
                    if surface == *window_wl_surface {
                        // Use the window immediatly when we are the toplevel surface.
                        // PointerFocusTarget::Window to proceed (namely
                        // State::process_mouse_action).
                        (
                            PointerFocusTarget::Window(fullscreen.clone()),
                            fullscreen_loc.to_f64(), // window loc is already global
                        )
                    } else {
                        (
                            PointerFocusTarget::from(surface),
                            (surface_loc + fullscreen_loc).to_f64(), /* window loc is already
                                                                      * global */
                        )
                    }
                })
            {
                return Some(ret);
            }
        }

        if let Some(layer) = layer_map.layer_under(Layer::Top, point_in_output) {
            let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
            if let Some((surface, surface_loc)) =
                layer.surface_under(point_in_output - layer_loc.to_f64(), WindowSurfaceType::ALL)
            {
                return Some((
                    PointerFocusTarget::WlSurface(surface),
                    (surface_loc + output_loc + layer_loc).to_f64(),
                ));
            }
        }

        if let Some((window, window_loc)) = self.space.window_under(point) {
            let render_offset = window.render_offset();
            let window_wl_surface = window.wl_surface().unwrap();
            // NOTE: window location passed here is already global, since its from
            // `Fht::window_geometry`
            if let Some(ret) = window
                .surface_under(
                    point_in_output - window_loc.to_f64(),
                    WindowSurfaceType::ALL,
                )
                .map(|(surface, surface_loc)| {
                    if surface == *window_wl_surface {
                        // Use the window immediatly when we are the toplevel surface.
                        // PointerFocusTarget::Window to proceed (namely
                        // State::process_mouse_action).
                        (
                            PointerFocusTarget::Window(window.clone()),
                            window_loc.to_f64(), // window loc is already global
                        )
                    } else {
                        (
                            PointerFocusTarget::from(surface),
                            (surface_loc + window_loc).to_f64(), /* window loc is already global */
                        )
                    }
                })
            {
                return Some(ret);
            }
        }

        if let Some(layer) = layer_map
            .layer_under(Layer::Bottom, point)
            .or_else(|| layer_map.layer_under(Layer::Background, point))
        {
            let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
            if let Some((surface, surface_loc)) =
                layer.surface_under(point_in_output - layer_loc.to_f64(), WindowSurfaceType::ALL)
            {
                return Some((
                    PointerFocusTarget::WlSurface(surface),
                    (surface_loc + output_loc + layer_loc).to_f64(),
                ));
            }
        }

        None
    }

    pub fn visible_output_for_surface(&self, surface: &WlSurface) -> Option<&Output> {
        for output in self.space.outputs() {
            // Lock surface and layer shells take priority.
            let output_state = OutputState::get(output);
            if output_state
                .lock_surface
                .as_ref()
                .is_some_and(|lock_surface| lock_surface.wl_surface() == surface)
            {
                drop(output_state); // avoid deadlocks
                return Some(output);
            }

            let layer_map = layer_map_for_output(output);
            if layer_map
                .layer_for_surface(surface, WindowSurfaceType::ALL)
                .is_some()
            {
                return Some(output);
            }
        }

        self.space.output_for_surface(surface)
    }

    pub fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some((window, workspace)) = self.space.find_window_and_workspace(&root) else {
            return;
        };

        // The target (aka the popup) geometry should be relative to the parent (aka the window's)
        // geometry, based on the xdg_shell protocol requirements.
        let mut target = workspace.output().geometry();
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= workspace.window_location(&window).unwrap();

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    pub fn advance_animations(&mut self, output: &Output, now: std::time::Duration) -> bool {
        let monitor = self
            .space
            .monitor_mut_for_output(output)
            .expect("all outputs should be tracked by Space");
        let mut ret = false;
        ret |= self
            .config_ui
            .advance_animations(now, !self.config.animations.disable);
        ret |= monitor.advance_animations(now);
        ret
    }
}
