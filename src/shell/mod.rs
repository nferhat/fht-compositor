pub mod cursor;
pub mod focus_target;
pub mod grabs;
pub mod window;
pub mod workspaces;

use std::cell::RefCell;

use grabs::{PointerResizeSurfaceGrab, ResizeData, ResizeState};
use smithay::desktop::{
    find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output, LayerSurface, PopupKind, Window, WindowSurfaceType
};
use smithay::input::pointer::{CursorImageStatus, Focus};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Monotonic, Point, Rectangle, Serial, Time};
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::Layer;
use smithay::wayland::shell::xdg::{PopupSurface, XdgToplevelSurfaceData};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as XdgToplevelState;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use workspaces::FullscreenTile;

pub use self::focus_target::{KeyboardFocusTarget, PointerFocusTarget};
use self::grabs::MoveSurfaceGrab;
use self::workspaces::tile::{WorkspaceElement, WorkspaceTile};
use self::workspaces::{Workspace, WorkspaceSwitchAnimation};
use crate::config::CONFIG;
use crate::state::{Fht, UnmappedTile};
use crate::utils::RectCenterExt;
use crate::utils::output::OutputExt;

impl Fht {
    /// Get the [`FocusTarget`] under the cursor.
    ///
    /// It checks the surface under the cursor using the following order:
    /// - [`Overlay`] layer shells.
    /// - [`Fullscreen`] windows on the active workspace.
    /// - [`Top`] layer shells.
    /// - Normal/Maximized windows on the active workspace.
    /// - [`Bottom`] layer shells.
    /// - [`Background`] layer shells.
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

        let window_surface_under = |(window, mut loc): (&Window, Point<i32, Logical>)| {
            loc -= window.geometry().loc; // the passed in location is without the window view
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
        } else if let Some(window_focus) = wset.element_under(point).and_then(window_surface_under)
        {
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

    /// Find the window associated with this [`WlSurface`]
    pub fn find_window(&self, surface: &WlSurface) -> Option<&Window> {
        self.workspaces()
            .find_map(|(_, wset)| wset.find_element(surface))
    }

    /// Find the tile associated with this [`WlSurface`]
    pub fn find_tile(&mut self, surface: &WlSurface) -> Option<&mut WorkspaceTile<Window>> {
        self.workspaces_mut()
            .find_map(|(_, wset)| wset.find_tile_mut(surface))
    }

    /// Find the window associated with this [`WlSurface`]
    pub fn find_window_and_workspace(
        &self,
        surface: &WlSurface,
    ) -> Option<(Window, &Workspace<Window>)> {
        self.workspaces()
            .find_map(|(_, wset)| wset.find_element_and_workspace(surface))
    }

    /// Find the window associated with this [`WlSurface`]
    pub fn find_window_and_workspace_mut(
        &mut self,
        surface: &WlSurface,
    ) -> Option<(Window, &mut Workspace<Window>)> {
        self.workspaces_mut()
            .find_map(|(_, wset)| wset.find_element_and_workspace_mut(surface))
    }

    /// Find the window associated with this [`WlSurface`], and the output the window is mapped
    /// onto
    pub fn find_window_and_output(&self, surface: &WlSurface) -> Option<(&Window, &Output)> {
        self.workspaces()
            .find_map(|(_, wset)| wset.find_element(surface).map(|w| (w, &wset.output)))
    }

    /// Find the tile associated with this [`WlSurface`]
    pub fn find_tile_and_output(
        &mut self,
        surface: &WlSurface,
    ) -> Option<(&mut WorkspaceTile<Window>, Output)> {
        self.workspaces_mut().find_map(|(_, wset)| {
            let output = wset.output.clone();
            wset.find_tile_mut(surface).map(|tile| (tile, output))
        })
    }

    /// Get a reference to the workspace holding this window
    pub fn ws_for(&self, window: &Window) -> Option<&Workspace<Window>> {
        self.workspaces().find_map(|(_, wset)| wset.ws_for(window))
    }

    /// Get a mutable reference to the workspace holding this window
    pub fn ws_mut_for(&mut self, window: &Window) -> Option<&mut Workspace<Window>> {
        self.workspaces_mut()
            .find_map(|(_, wset)| wset.ws_mut_for(window))
    }

    /// Get a this window's geometry in global coordinate space.
    pub fn window_geometry(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        self.workspaces().find_map(|(_, wset)| {
            wset.ws_for(window)
                .and_then(|ws| ws.element_geometry(window))
        })
    }

    /// Get a this window's geometry in global coordinate space.
    pub fn window_visual_geometry(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        self.workspaces().find_map(|(_, wset)| {
            wset.ws_for(window)
                .and_then(|ws| ws.element_visual_geometry(window))
        })
    }

    /// Find the first output where this [`WlSurface`] is visible.
    ///
    /// This checks everything from layer shells to windows to override redirect windows etc.
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

    /// Find every window that is curently displayed on this output
    #[profiling::function]
    pub fn visible_windows_for_output(&self, output: &Output) -> impl Iterator<Item = &Window> {
        let wset = self.wset_for(output);

        let switching_windows = wset
            .switch_animation
            .as_ref()
            .map(|anim| {
                let ws = &wset.workspaces[anim.target_idx];

                ws.fullscreen
                    .as_ref()
                    .map(|fs| fs.inner.element())
                    .into_iter()
                    .chain(ws.tiles.iter().map(WorkspaceTile::element))
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .flatten();

        let active = wset.active();
        active
            .fullscreen
            .as_ref()
            .map(|fs| fs.inner.element())
            .into_iter()
            .chain(active.tiles.iter().map(WorkspaceTile::element))
            .chain(switching_windows)
    }

    /// Prepapre a pending window to be mapped.
    pub fn prepare_pending_window(&mut self, window: Window) {
        let mut output = self.focus_state.output.clone().unwrap();
        let toplevel = window.toplevel().unwrap();
        let wl_surface = toplevel.wl_surface();

        // Get the matching mapping setting, if the user specified one.
        let workspace_idx = self.wset_for(&output).get_active_idx();
        let (title, app_id) = with_states(&wl_surface, |states| {
            let data = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            (
                data.title.clone().unwrap_or_default(),
                data.app_id.clone().unwrap_or_default(),
            )
        });

        let map_settings = CONFIG
            .rules
            .iter()
            .find(|(rules, _)| {
                rules
                    .iter()
                    .any(|r| r.matches(&title, &app_id, workspace_idx))
            })
            .map(|(_, settings)| settings.clone())
            .unwrap_or_default();

        // Apply rules
        //
        // First start with the output since every operation (mapping,  fullscreening, etc...) will
        // be done relative to the output.
        if let Some(target_output) = map_settings
            .output
            .as_ref()
            .and_then(|name| self.outputs().find(|o| o.name().as_str() == name))
            .cloned()
        {
            output = target_output;
        }

        let wset = self.wset_mut_for(&output);
        let mut workspace_idx = match map_settings.workspace {
            None => wset.get_active_idx(),
            Some(idx) => idx.clamp(0, 9),
        };

        // Even if the user set rules, we still always prefer the output and workspace of this
        // window's toplevel parent.
        //
        // You can for example imagine a gtk4 application opening a child window or prompt, for
        // example. Or, opening a new window from your browser main one.
        if let Some((parent_index, parent_output)) = toplevel.parent().and_then(|parent_surface| {
            self.workspaces().find_map(|(output, wset)| {
                let idx = wset
                    .workspaces()
                    .enumerate()
                    .position(|(_, ws)| ws.has_surface(&parent_surface))?;
                Some((idx, output.clone()))
            })
        }) {
            workspace_idx = parent_index;
            output = parent_output;
        }

        let wset = self.wset_mut_for(&output);
        let workspace = &mut wset.workspaces[workspace_idx];
        let layout = workspace.get_active_layout();

        // Pre compute window geometry for insertion.
        let mut tile = WorkspaceTile::new(window.clone(), None);
        let tile_area = workspace.tile_area();
        layout.arrange_tiles(
            workspace.tiles.iter_mut().chain(std::iter::once(&mut tile)),
            tile_area,
            CONFIG.general.inner_gaps,
            false,
        );

        // Client side-decorations
        let allow_csd = map_settings
            .allow_csd
            .unwrap_or(CONFIG.decoration.allow_csd);
        toplevel.with_pending_state(|state| {
            if allow_csd {
                state.decoration_mode = Some(DecorationMode::ClientSide);
            } else {
                state.decoration_mode = Some(DecorationMode::ServerSide);
                // For some reason clients still draw decorations even when asked not to.
                // Some dont if you set their state to tiled (wow)
                state.states.set(XdgToplevelState::TiledTop);
                state.states.set(XdgToplevelState::TiledLeft);
                state.states.set(XdgToplevelState::TiledRight);
                state.states.set(XdgToplevelState::TiledBottom);
            }
        });

        tile.element.toplevel().unwrap().send_configure();

        self.unmapped_tiles.push(UnmappedTile {
            inner: tile,
            last_output: Some(output),
            last_workspace_idx: Some(workspace_idx),
        })
    }

    /// Map a pending window, if it's found.
    ///
    /// Returns the output where this tile has been mapped.
    pub fn map_tile(&mut self, unmapped_tile: UnmappedTile) -> Output {
        let loop_handle = self.loop_handle.clone();

        // Make sure we have valid values before insertion.
        let UnmappedTile {
            inner: tile,
            last_output,
            last_workspace_idx,
        } = unmapped_tile;
        let wl_surface = tile.element().wl_surface().unwrap().into_owned();
        let output = last_output.unwrap_or_else(|| self.active_output());
        let output_loc = output.current_location();
        let wset = self.wset_mut_for(&output);
        let active_idx = wset.get_active_idx();
        let workspace_idx = last_workspace_idx.unwrap_or(active_idx);

        let is_active = workspace_idx == wset.get_active_idx();
        let workspace = &mut wset.workspaces[workspace_idx];

        let window = tile.element.clone();
        workspace.insert_tile(tile, false);

        let tile = workspace.find_tile_mut(&wl_surface).unwrap();
        tile.start_opening_animation();
        // we dont want to animate the tile now.
        tile.location_animation.take();
        let mut tile_geo = tile.geometry();
        tile_geo.loc += output_loc;

        // From using the compositor opening a window when a switch is being done feels more
        // natural when the window gets focus, even if focus_new_windows is none.
        let is_switching = wset.switch_animation.is_some();
        let should_focus = (CONFIG.general.focus_new_windows || is_switching) && is_active;

        if should_focus {
            let center = tile_geo.center();
            loop_handle.insert_idle(move |state| {
                if CONFIG.general.cursor_warps {
                    state.move_pointer(center.to_f64());
                }
                state.set_focus_target(Some(window.clone().into()));
            });
        }

        output
    }

    /// Unconstraint a popup.
    ///
    /// Basically changes its geometry and location so that it doesn't overflow outside of the
    /// parent window's output.
    pub fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some((window, workspace)) = self.find_window_and_workspace(&root) else {
            return;
        };

        // The target (aka the popup) geometry should be relative to the parent (aka the window's)
        // geometry, based on the xdg_shell protocol requirements.
        let mut target = workspace.output.geometry();
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= workspace.element_geometry(&window).unwrap().loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    /// Advance all the active animations for this given output
    pub fn advance_animations(&mut self, output: &Output, current_time: Time<Monotonic>) -> bool {
        let mut animations_running = false;
        let wset = self.wset_mut_for(output);
        if let Some(WorkspaceSwitchAnimation { target_idx, .. }) =
            wset.switch_animation.take_if(|a| a.animation.is_finished())
        {
            wset.active_idx
                .store(target_idx, std::sync::atomic::Ordering::SeqCst);
        }
        if let Some(animation) = wset.switch_animation.as_mut() {
            animation.animation.set_current_time(current_time);
            animations_running = true;
        }
        for ws in wset.workspaces_mut() {
            if let Some(FullscreenTile { inner, .. }) = ws.fullscreen.as_mut() {
                animations_running |= inner.advance_animations(current_time);
            }

            for window in &mut ws.tiles {
                animations_running |= window.advance_animations(current_time);
            }
        }

        animations_running
    }

    /// Get an interator over all the windows registered in the compositor.
    pub fn all_windows(&self) -> impl Iterator<Item = &Window> + '_ {
        self.workspaces.values().flat_map(|wset| {
            let workspaces = &wset.workspaces;
            workspaces
                .iter()
                .flat_map(|ws| ws.tiles.iter().map(|tile| tile.element()))
        })
    }
}

impl crate::state::State {
    /// Process a move request for this given window.
    pub fn handle_move_request(&mut self, window: Window, serial: Serial) {
        // NOTE: About internal handling.
        // ---
        // Even though `XdgShellHandler::move_request` has a seat argument, we only advertise one
        // single seat to clients (why would we support multi-seat for a standalone compositor?)
        // So the only pointer we have is the advertised seat pointer.
        let pointer = self.fht.pointer.clone();
        if !pointer.has_grab(serial) {
            return;
        }

        let mut start_data = pointer.grab_start_data().unwrap();
        start_data.focus = None;

        let mut window_geo = self.fht.window_geometry(&window).unwrap();

        // Unmaximize/Unfullscreen if it already is.
        let is_maximized = window.maximized();
        let is_fullscreen = window.fullscreen();
        window.set_maximized(false);
        window.set_fullscreen(false);
        window.set_fullscreen_output(None);

        if is_maximized || is_fullscreen {
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_configure();
            }

            // let pos = pointer.current_location().as_global();
            // let mut window_pos = pos - window_geo.to_f64().loc;
            // window_pos.x = window_pos.x.clamp(0.0, window_geo.size.w.to_f64());
            //
            // match window_pos.x / window_geo.size.w.to_f64() {
            //     x if x < 0.5
            // }
            let pos = pointer.current_location();
            window_geo.loc = (pos.x as i32, pos.y as i32).into();
        }

        let grab = MoveSurfaceGrab::new(start_data, window, window_geo);

        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    /// Process a resize request for this given window.
    pub fn handle_resize_request(
        &mut self,
        window: Window,
        serial: Serial,
        edges: grabs::ResizeEdge,
    ) {
        // NOTE: About internal handling.
        // ---
        // Even though `XdgShellHandler::move_request` has a seat argument, we only advertise one
        // single seat to clients (why would we support multi-seat for a standalone compositor?)
        // So the only pointer we have is the advertised seat pointer.
        let pointer = self.fht.pointer.clone();
        if !pointer.has_grab(serial) {
            return;
        }
        let mut start_data = pointer.grab_start_data().unwrap();
        start_data.focus = None;

        let Some(wl_surface) = window.wl_surface() else {
            return;
        };

        let mut window_geo = self.fht.window_geometry(&window).unwrap();

        // Unmaximize/Unfullscreen if it already is.
        let is_maximized = window.maximized();
        let is_fullscreen = window.fullscreen();
        window.set_maximized(false);
        window.set_fullscreen(false);
        window.set_fullscreen_output(None);

        if is_maximized || is_fullscreen {
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_configure();
            }

            // let pos = pointer.current_location().as_global();
            // let mut window_pos = pos - window_geo.to_f64().loc;
            // window_pos.x = window_pos.x.clamp(0.0, window_geo.size.w.to_f64());
            //
            // match window_pos.x / window_geo.size.w.to_f64() {
            //     x if x < 0.5
            // }
            let pos = pointer.current_location();
            window_geo.loc = (pos.x as i32, pos.y as i32).into();
        }

        with_states(&wl_surface, move |states| {
            let mut state = states
                .data_map
                .get_or_insert(|| RefCell::new(ResizeState::default()))
                .borrow_mut();
            *state = ResizeState::Resizing(ResizeData {
                edges: edges.into(),
                initial_window_location: window_geo.loc,
                initial_window_size: window_geo.size,
            });
        });

        self.fht.loop_handle.insert_idle(move |state| {
            // Set the cursor icon.
            let icon = edges.cursor_icon();
            let mut lock = state.fht.cursor_theme_manager.image_status.lock().unwrap();
            *lock = CursorImageStatus::Named(icon);
            state.fht.resize_grab_active = true;
        });

        let grab = PointerResizeSurfaceGrab::new(start_data, window, edges, window_geo.size);

        pointer.set_grab(self, grab, serial, Focus::Clear);
    }
}
