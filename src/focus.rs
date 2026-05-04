//! # `focus_target` - focus handling for `fhtc`
//!
//! To smithay, we expose only [`WlSurface`] objects as the focused keyboard/mouse/touch targets.
//! However, internally (IE. in the compositor state), we keep track of where that [`WlSurface`]
//! came from.

use std::borrow::Cow;

use smithay::desktop::utils::under_from_surface_tree;
use smithay::desktop::{layer_map_for_output, LayerSurface, WindowSurfaceType};
use smithay::input::pointer::MotionEvent;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive as _, Logical, Point, SERIAL_COUNTER};
use smithay::wayland::session_lock::LockSurface;
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer};

use crate::output::OutputExt;
use crate::state::{Fht, State};
use crate::utils::get_monotonic_time;
use crate::window::Window;

#[derive(Debug, Clone)]
pub enum KeyboardFocus {
    /// We are focused on the [`Space`](crate::space::Space). This is the default focus if there's
    /// nothing else capturing focus (in which case the window will be `None`)
    ///
    /// This is not a [`Window`](crate::window::Window) since we might be focused on a popup of
    /// a window.
    Space { surface: Option<WlSurface> },
    /// We are focusing a given [`LayerSurface`]
    LayerSurface { layer: LayerSurface },
    /// We are focused on a [`LockSurface`].
    LockSurface { lock: LockSurface },
}

impl KeyboardFocus {
    pub fn wl_surface<'a>(&'a self) -> Option<Cow<'a, WlSurface>> {
        match self {
            KeyboardFocus::Space { surface } => surface.as_ref().map(Cow::Borrowed),
            KeyboardFocus::LayerSurface { layer, .. } => Some(Cow::Borrowed(layer.wl_surface())),
            KeyboardFocus::LockSurface { lock } => Some(Cow::Borrowed(lock.wl_surface())),
        }
    }

    pub fn into_wl_surface(self) -> Option<WlSurface> {
        match self {
            KeyboardFocus::Space { surface } => surface,
            KeyboardFocus::LayerSurface { layer, .. } => Some(layer.wl_surface().clone()),
            KeyboardFocus::LockSurface { lock } => Some(lock.wl_surface().clone()),
        }
    }
}

/// Calculated contents that are below the pointer. Calculated using [`Fht::get_pointer_focus`].
#[derive(Debug)]
pub struct PointerFocus {
    /// The output that contains the pointer.
    pub output: Output,
    /// The surface under the pointer, and it's location in global space.
    pub surface: Option<(WlSurface, Point<f64, Logical>)>,
    /// The window under the pointer.
    pub window: Option<Window>,
    /// The layer surface under the pointer.
    pub layer_surface: Option<LayerSurface>,
}

impl State {
    /// Updates the keyboard focus.
    ///
    /// If the compositor is locked, it will try to focus the current lock surface on the active
    /// output, otherwise it will force disable keyboard interaction with any other surfaces.
    ///
    /// If the compositor is not locked, it will check in the following order.
    ///
    /// 1. Overlay layer shells (exclusive then on-demand)
    /// 2. Fullscreened window on the active workspace.
    /// 3. Top layer shells (exclusive then on-demand)
    /// 4. Focused window
    /// 5. Bottom layer shells (on-demand only)
    /// 6. Background layer shells (on-demand only)
    ///
    /// You should maintain this order for consistency in other parts of the compositor (like for
    /// example if you need to calculate manually which surface to focus), otherwise you WILL
    /// get weird bugs with the keyboard focus.
    pub fn update_keyboard_focus(&mut self) {
        crate::profile_function!();
        let keyboard = self.fht.keyboard.clone();
        let output = self.fht.space.active_output().clone();

        // Before updating keyboard focus, make sure the layer-shell the user requested to focus
        // (by clicking) can still accept keyboard focus
        _ = self
            .fht
            .focused_on_demand_layer_shell
            .take_if(|layer_shell| {
                if !layer_shell.alive() {
                    return false; // dead, byebye
                }

                let keyboard_interactivity = layer_shell.cached_state().keyboard_interactivity;
                !matches!(
                    keyboard_interactivity,
                    KeyboardInteractivity::Exclusive | KeyboardInteractivity::OnDemand
                )
            });

        let new_focus = if self.fht.is_locked() {
            let output_state = self.fht.output_state.get(&output).unwrap();
            if let Some(lock) = output_state.lock_surface.clone() {
                Some(KeyboardFocus::LockSurface { lock })
            } else {
                // Even if the compositor isn't locked we force remove the focus from everything
                // else here, since we might in a state when the lock program didn't assign surfaces
                // yet
                None
            }
        } else {
            let mon = self.fht.space.monitor_for_output(&output).unwrap();

            // When checking for window focus, the fullscreened window always take precedence,
            // since its the only one displayed.
            let focused_window = || {
                mon.active_workspace()
                    .active_window()
                    .map(|win| KeyboardFocus::Space {
                        surface: Some(win.wl_surface().clone()),
                    })
            };
            let fullscreen_window_on_monitor = || {
                mon.active_workspace()
                    .fullscreened_window()
                    .map(|win| KeyboardFocus::Space {
                        surface: Some(win.wl_surface().clone()),
                    })
            };

            // When checking for layer shell focus, exclusive keyboard focus obviously takes the
            // precedence, then we check on-demand.
            //
            // On-demand layer-shells get keyboard focus only when they get pressed down.
            let layer_map = layer_map_for_output(&output);
            let on_demand_layer_shell = |layer| {
                layer_map
                    .layers_on(layer)
                    .find(|&layer| Some(layer) == self.fht.focused_on_demand_layer_shell.as_ref())
                    .cloned()
                    .map(|layer| KeyboardFocus::LayerSurface { layer })
            };
            let exclusive_layer_shell = |layer| {
                layer_map
                    .layers_on(layer)
                    .find(|&layer| {
                        layer.cached_state().keyboard_interactivity
                            == KeyboardInteractivity::Exclusive
                    })
                    .cloned()
                    .map(|layer| KeyboardFocus::LayerSurface { layer })
            };
            let layer_shell_focus =
                |layer| exclusive_layer_shell(layer).or_else(|| on_demand_layer_shell(layer));

            // Now start checking for focus, from Overlay layer shells
            //
            // Make sure that these are ordered the same way in Fht::output_elements to ensure
            // consistency.
            let mut ft = layer_shell_focus(Layer::Overlay);
            if mon.render_above_top() {
                ft = ft.or_else(|| fullscreen_window_on_monitor());
                ft = ft.or_else(|| focused_window());
                ft = ft.or_else(|| layer_shell_focus(Layer::Top));
                ft = ft.or_else(|| layer_shell_focus(Layer::Bottom));
                ft = ft.or_else(|| layer_shell_focus(Layer::Background));
            } else {
                ft = ft.or_else(|| layer_shell_focus(Layer::Top));
                ft = ft.or_else(|| fullscreen_window_on_monitor());
                ft = ft.or_else(|| focused_window());
                ft = ft.or_else(|| layer_shell_focus(Layer::Bottom));
                ft = ft.or_else(|| layer_shell_focus(Layer::Background));
            }

            ft
        };

        let focus_surface = new_focus.and_then(|f| f.into_wl_surface());

        if keyboard.current_focus() != focus_surface {
            // Inform the workspace system about the new focus, this will in turn set the Activated
            // xdg_toplevel state on the window (after State::dispatch)
            // if let Some() = &new_focus {
            //     if !self.fht.space.activate_window(window, true) {
            //         // Don't really know when this can hapen
            //         error!("Window from space disappeared while being focused");
            //         return;
            //     }
            // }

            // FIXME: We are not handling popup grabs here, might mess things here.
            //
            // By default anvil early returns on this function if the keyboard/pointer are grabbed,
            // but seems like a hack more like anything else
            self.set_keyboard_focus(focus_surface);
        }
    }

    /// Refresh the pointer focus.
    pub fn update_pointer_focus(&mut self) {
        crate::profile_scope!("refresh_pointer_focus");
        // We try to update the pointer focus. If the new one is not the same as the previous one we
        // encountered, we send a motion and frame event to the new one.
        let pointer = self.fht.pointer.clone();
        let pointer_loc = pointer.current_location();
        let Some(focus) = self.fht.get_pointer_focus(pointer_loc) else {
            return;
        };

        pointer.motion(
            self,
            focus.surface,
            &MotionEvent {
                location: pointer_loc,
                serial: SERIAL_COUNTER.next_serial(),
                time: get_monotonic_time().as_millis() as u32,
            },
        );
        // After motion, try to activate new pointer constraint under surface
        self.fht.activate_pointer_constraint();

        pointer.frame(self);
    }
}

impl Fht {
    /// Calculates and returns the current [`PointerFocus`]
    pub fn get_pointer_focus(&self, loc: Point<f64, Logical>) -> Option<PointerFocus> {
        let output = self
            .space
            .outputs()
            .find(|o| o.geometry().to_f64().contains(loc))
            .cloned()?;
        let output_loc = output.current_location();
        // Needed since the workspaces expect output-local coordinates
        let loc_in_output = loc - output_loc.to_f64();

        let mut rv = PointerFocus {
            output,
            layer_surface: None,
            surface: None,
            window: None,
        };

        if self.is_locked() {
            // Only focus the lock surface.
            let output_state = self.output_state.get(&rv.output).unwrap();
            let Some(lock_surface) = &output_state.lock_surface else {
                // This can happen if the lock program didn't submit a surface for this output yet.
                return Some(rv);
            };

            let surface = under_from_surface_tree(
                lock_surface.wl_surface(),
                loc_in_output,
                // We put lock surfaces at (0, 0).
                (0, 0),
                WindowSurfaceType::ALL,
            )
            .map(|(surface, pos_within_output)| {
                (surface, (pos_within_output + output_loc).to_f64())
            });

            rv.surface = surface;
            return Some(rv);
        }

        let layer_map = layer_map_for_output(&rv.output);
        let layer_under = |layer| {
            layer_map
                .layers_on(layer)
                .rev()
                .find_map(|layer_surface| {
                    let layer_loc = layer_map.layer_geometry(layer_surface).unwrap().loc;
                    layer_surface
                        .surface_under(loc_in_output - layer_loc.to_f64(), WindowSurfaceType::ALL)
                        .map(|(surface, pos_within_layer)| {
                            (
                                (surface, pos_within_layer + output_loc + layer_loc),
                                layer_surface,
                            )
                        })
                })
                .map(|(s, l)| (Some(s), (None, Some(l.clone()))))
        };

        let window_under = |fullscreen| {
            let maybe_window = if fullscreen {
                self.space.fullscreened_window_under(loc)
            } else {
                self.space.window_under(loc)
            };

            maybe_window
                .and_then(|(window, window_loc)| {
                    window
                        .surface_under(loc_in_output - window_loc.to_f64(), WindowSurfaceType::ALL)
                        .map(|(surface, pos_within_window)| {
                            (
                                (surface, pos_within_window + window_loc + output_loc),
                                window,
                            )
                        })
                })
                .map(|(s, w)| (Some(s), (Some(w), None)))
        };

        let monitor = self.space.monitor_for_output(&rv.output).unwrap();
        let render_above_top = monitor.render_above_top();

        let mut under = layer_under(Layer::Overlay);
        if render_above_top {
            under = under.or_else(|| window_under(true));
            under = under.or_else(|| layer_under(Layer::Top));
            under = under.or_else(|| window_under(false));
        } else {
            under = under.or_else(|| layer_under(Layer::Top));
            under = under.or_else(|| window_under(true));
            under = under.or_else(|| window_under(false));
        }

        under = under.or_else(|| layer_under(Layer::Bottom));
        under = under.or_else(|| layer_under(Layer::Background));

        // Drop this in order to release the borrow on &rv.output
        drop(layer_map);

        let Some((surface, (window, layer))) = under else {
            return Some(rv);
        };

        rv.surface = surface.map(|(s, loc)| (s, loc.to_f64()));
        rv.window = window;
        rv.layer_surface = layer;

        Some(rv)
    }
}
