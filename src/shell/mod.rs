pub mod cursor;
pub mod focus_target;
pub mod workspaces;

use smithay::desktop::{
    find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output, LayerSurface,
    PopupKind, WindowSurfaceType,
};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Monotonic, Point, Rectangle, Time};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::Layer;
use smithay::wayland::shell::xdg::PopupSurface;
use workspaces::WorkspaceId;

pub use self::focus_target::{KeyboardFocusTarget, PointerFocusTarget};
use self::workspaces::Workspace;
use crate::state::Fht;
use crate::utils::output::OutputExt;
use crate::window::Window;

impl Fht {
    pub fn focus_target_under(
        &self,
        mut point: Point<f64, Logical>,
    ) -> Option<(PointerFocusTarget, Point<f64, Logical>)> {
        let output = self.focus_state.output.as_ref()?;
        let output_loc = output.current_location();
        point -= output_loc.to_f64();

        let wset = self.wset_for(output);
        let layer_map = layer_map_for_output(output);

        let mut under = None;

        let layer_surface_under = |layer: &LayerSurface, loc: Point<i32, Logical>| {
            layer
                .surface_under(
                    point - output_loc.to_f64() - loc.to_f64(),
                    WindowSurfaceType::ALL,
                )
                .map(|(surface, surface_loc)| {
                    (
                        PointerFocusTarget::from(surface),
                        (surface_loc + output_loc + loc).to_f64(),
                    )
                })
        };

        let window_surface_under = |(window, mut loc): (Window, Point<i32, Logical>)| {
            loc -= window.render_offset(); // the passed in location is without the window view
                                           // offset
            let window_wl_surface = window.wl_surface().unwrap();
            // NOTE: window location passed here is already global, since its from
            // `Fht::window_geometry`
            window
                .surface_under(point - loc.to_f64(), WindowSurfaceType::ALL)
                .map(|(surface, surface_loc)| {
                    if surface == *window_wl_surface {
                        // Use the window immediatly when we are the toplevel surface.
                        // PointerFocusTarget::Window to proceed (namely
                        // State::process_mouse_action).
                        (
                            PointerFocusTarget::Window(window.clone()),
                            loc.to_f64(), // window loc is already global
                        )
                    } else {
                        (
                            PointerFocusTarget::from(surface),
                            (surface_loc + loc).to_f64(), /* window loc is already global */
                        )
                    }
                })
        };

        if let Some(layer_focus) = layer_map
            .layer_under(Layer::Overlay, point)
            .and_then(|layer| {
                let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
                layer_surface_under(layer, layer_loc)
            })
        {
            under = Some(layer_focus);
        } else if let Some(fullscreen_focus) =
            wset.current_fullscreen().and_then(window_surface_under)
        {
            under = Some(fullscreen_focus)
        } else if let Some(layer_focus) =
            layer_map.layer_under(Layer::Top, point).and_then(|layer| {
                let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
                layer_surface_under(layer, layer_loc)
            })
        {
            under = Some(layer_focus)
        } else if let Some(window_focus) = wset.window_under(point).and_then(window_surface_under) {
            under = Some(window_focus)
        } else if let Some(layer_focus) = layer_map
            .layer_under(Layer::Bottom, point)
            .or_else(|| layer_map.layer_under(Layer::Background, point))
            .and_then(|layer| {
                let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
                layer_surface_under(layer, layer_loc)
            })
        {
            under = Some(layer_focus)
        }

        under
    }

    pub fn find_window(&self, surface: &WlSurface) -> Option<Window> {
        self.workspaces()
            .find_map(|(_, wset)| wset.find_window(surface))
    }

    pub fn find_window_and_workspace(&self, surface: &WlSurface) -> Option<(Window, &Workspace)> {
        self.workspaces()
            .find_map(|(_, wset)| wset.find_window_and_workspace(surface))
    }

    pub fn find_window_and_workspace_mut(
        &mut self,
        surface: &WlSurface,
    ) -> Option<(Window, &mut Workspace)> {
        self.workspaces_mut()
            .find_map(|(_, wset)| wset.find_window_and_workspace_mut(surface))
    }

    pub fn find_window_and_output(&self, surface: &WlSurface) -> Option<(Window, Output)> {
        self.workspaces()
            .find_map(|(_, wset)| wset.find_window(surface).map(|w| (w, wset.output())))
    }

    pub fn workspace_for_window(&self, window: &Window) -> Option<&Workspace> {
        self.workspaces()
            .find_map(|(_, wset)| wset.workspace_for_window(window))
    }

    pub fn workspace_for_window_mut(&mut self, window: &Window) -> Option<&mut Workspace> {
        self.workspaces_mut()
            .find_map(|(_, wset)| wset.workspace_mut_for_window(window))
    }

    pub fn window_geometry(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        self.workspaces().find_map(|(_, wset)| {
            wset.workspace_for_window(window)
                .and_then(|ws| ws.window_geometry(window))
        })
    }

    pub fn get_workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces_mut()
            .find_map(|(_, wset)| wset.workspaces_mut().find(|ws| ws.id() == id))
    }

    pub fn window_visual_geometry(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        self.workspaces().find_map(|(_, wset)| {
            wset.workspace_for_window(window)
                .and_then(|ws| ws.window_visual_geometry(window))
        })
    }

    pub fn visible_output_for_surface(&self, surface: &WlSurface) -> Option<&Output> {
        self.outputs()
            .find(|o| {
                // Is the surface a layer shell?
                let layer_map = layer_map_for_output(o);
                layer_map
                    .layer_for_surface(surface, WindowSurfaceType::ALL)
                    .is_some()
            })
            .or_else(|| {
                // Mapped window?
                self.workspaces().find_map(|(o, wset)| {
                    let active = wset.active();
                    if active.has_surface(surface) {
                        return Some(o);
                    }

                    None
                })
            })
    }

    pub fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some((window, workspace)) = self.find_window_and_workspace(&root) else {
            return;
        };

        // The target (aka the popup) geometry should be relative to the parent (aka the window's)
        // geometry, based on the xdg_shell protocol requirements.
        let mut target = workspace.output().geometry();
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= workspace.window_geometry(&window).unwrap().loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    pub fn advance_animations(&mut self, output: &Output, current_time: Time<Monotonic>) -> bool {
        let wset = self.wset_mut_for(output);
        wset.advance_animations(current_time)
    }

    pub fn all_windows(&self) -> impl Iterator<Item = &Window> + '_ {
        self.workspaces.values().flat_map(|wset| {
            wset.workspaces()
                .flat_map(|ws| ws.tiles().map(|tile| tile.window()))
        })
    }
}
