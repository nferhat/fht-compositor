pub mod cursor;
pub mod focus_target;
pub mod grabs;
pub mod window;
pub mod workspaces;

use smithay::desktop::{
    find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output, LayerSurface, PopupKind, Window, WindowSurfaceType
};
use smithay::input::pointer::Focus;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Monotonic, Point, Rectangle, Serial, Time};
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::Layer;
use smithay::wayland::shell::xdg::{PopupSurface, XdgToplevelSurfaceData};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as XdgToplevelState;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;

pub use self::focus_target::{KeyboardFocusTarget, PointerFocusTarget};
use self::grabs::MoveSurfaceGrab;
use self::workspaces::tile::{WorkspaceElement, WorkspaceTile};
use self::workspaces::{Workspace, WorkspaceSwitchAnimation};
use crate::config::CONFIG;
use crate::state::{Fht, UnmappedTile};
use crate::utils::geometry::{
    Global, PointExt, PointGlobalExt, PointLocalExt, RectCenterExt, RectExt, RectGlobalExt, RectLocalExt,
};
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
        point: Point<f64, Global>,
    ) -> Option<(PointerFocusTarget, Point<i32, Global>)> {
        let output = self.focus_state.output.as_ref()?;
        let wset = self.wset_for(output);
        let layer_map = layer_map_for_output(output);

        let mut under = None;

        let layer_surface_under = |layer: &LayerSurface, loc: Point<i32, Logical>| {
            layer
                .surface_under(
                    point.to_local(output).as_logical() - loc.to_f64(),
                    WindowSurfaceType::ALL,
                )
                .map(|(surface, surface_loc)| {
                    (
                        PointerFocusTarget::from(surface),
                        (surface_loc + loc).as_local().to_global(output),
                    )
                })
        };

        let window_surface_under = |window: &Window, loc: Point<i32, Logical>| {
            let window_wl_surface = window.wl_surface().unwrap();
            window
                .surface_under(point.as_logical() - loc.to_f64(), WindowSurfaceType::ALL)
                .map(|(surface, surface_loc)| {
                    if surface == window_wl_surface {
                        // Use the window immediatly when we are the toplevel surface.
                        // PointerFocusTarget::Window to proceed (namely
                        // State::process_mouse_action).
                        (
                            PointerFocusTarget::Window(window.clone()),
                            loc.as_global(), // window loc is already global
                        )
                    } else {
                        (
                            PointerFocusTarget::from(surface),
                            (surface_loc + loc).as_global(), // window loc is already global
                        )
                    }
                })
        };

        if let Some(layer_focus) = layer_map
            .layer_under(Layer::Overlay, point.as_logical())
            .and_then(|layer| {
                let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
                layer_surface_under(layer, layer_loc)
            })
        {
            under = Some(layer_focus);
        } else if let Some(fullscreen_focus) = wset
            .current_fullscreen()
            .and_then(|(fullscreen, loc)| window_surface_under(fullscreen, loc.as_logical()))
        {
            under = Some(fullscreen_focus)
        } else if let Some(layer_focus) = layer_map
            .layer_under(Layer::Top, point.as_logical())
            .and_then(|layer| {
                let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
                layer_surface_under(layer, layer_loc)
            })
        {
            under = Some(layer_focus)
        } else if let Some(window_focus) = wset
            .element_under(point)
            .and_then(|(window, loc)| window_surface_under(window, loc.as_logical()))
        {
            under = Some(window_focus)
        } else if let Some(layer_focus) = layer_map
            .layer_under(Layer::Bottom, point.as_logical())
            .or_else(|| layer_map.layer_under(Layer::Background, point.as_logical()))
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

    /// Find the window associated with this [`WlSurface`]
    pub fn find_window_and_workspace(
        &self,
        surface: &WlSurface,
    ) -> Option<(&Window, &Workspace<Window>)> {
        self.workspaces()
            .find_map(|(_, wset)| wset.find_element_and_workspace(surface))
    }

    /// Find the window associated with this [`WlSurface`], and the output the window is mapped
    /// onto
    pub fn find_window_and_output(&self, surface: &WlSurface) -> Option<(&Window, &Output)> {
        self.workspaces()
            .find_map(|(_, wset)| wset.find_element(surface).map(|w| (w, &wset.output)))
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

    /// Get a this window's geometry.
    pub fn window_geometry(&self, window: &Window) -> Option<Rectangle<i32, Global>> {
        self.workspaces().find_map(|(_, wset)| {
            wset.ws_for(window)
                .and_then(|ws| ws.element_geometry(window))
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
                    if active
                        .tiles
                        .iter()
                        .any(|tile| tile.has_surface(surface, WindowSurfaceType::ALL))
                    {
                        return Some(o);
                    }

                    // if active
                    //     .fullscreen
                    //     .as_ref()
                    //     .is_some_and(|f| f.inner.has_surface(surface, WindowSurfaceType::ALL))
                    // {
                    //     return Some(o);
                    // }

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
                wset.workspaces[anim.target_idx]
                    .tiles
                    .iter()
                    .map(WorkspaceTile::element)
            })
            .into_iter()
            .flatten();

        wset.active()
            .tiles
            .iter()
            .map(WorkspaceTile::element)
            .chain(switching_windows)
            .into_iter()
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
        if let Some((_, parent_workspace)) = toplevel
            .parent()
            .and_then(|parent_surface| self.find_window_and_workspace(&parent_surface))
        {
            workspace_idx = parent_workspace.index;
            output = parent_workspace.output.clone();
        }

        let wset = self.wset_mut_for(&output);
        let workspace = &mut wset.workspaces[workspace_idx];
        let layout = workspace.get_active_layout();

        // Pre compute window geometry for insertion.
        let mut tile = WorkspaceTile::new(window.clone(), None);
        let inner_gaps = CONFIG.general.inner_gaps;
        let outer_gaps = CONFIG.general.outer_gaps;

        let usable_geo = layer_map_for_output(&wset.output)
            .non_exclusive_zone()
            .as_local();
        let mut tile_area = usable_geo;
        tile_area.size -= (2 * outer_gaps, 2 * outer_gaps).into();
        tile_area.loc += (outer_gaps, outer_gaps).into();

        let tiles_len = workspace.tiles.len() + 1;
        layout.arrange_tiles(
            workspace.tiles.iter_mut().chain(std::iter::once(&mut tile)),
            tiles_len,
            tile_area,
            inner_gaps,
        );

        // We dont want to animate the movement of opening windows.
        tile.location_animation = None;

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
        let wl_surface = tile.element().wl_surface().unwrap();
        let output = last_output.unwrap_or_else(|| self.active_output());
        let wset = self.wset_mut_for(&output);
        let active_idx = wset.get_active_idx();
        let workspace_idx = last_workspace_idx.unwrap_or(active_idx);

        let is_active = workspace_idx == wset.get_active_idx();
        let workspace = &mut wset.workspaces[workspace_idx];

        let window = tile.element.clone();
        workspace.insert_tile(tile);

        let tile = workspace.find_tile(&wl_surface).unwrap();
        // we dont want to animate the tile now.
        tile.location_animation.take();
        let tile_geo = tile.geometry().to_global(&output);

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
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone())).as_global();
        target.loc -= workspace.element_location(window).unwrap();

        popup.with_pending_state(|state| {
            state.geometry = state
                .positioner
                .get_unconstrained_geometry(target.as_logical());
        });
    }

    /// Advance all the active animations for this given output
    pub fn advance_animations(&mut self, output: &Output, current_time: Time<Monotonic>) -> bool {
        // First check, egui running, since it may be running animations + update the overlay
        let mut animations_running = self.egui.active;
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
        for tile in wset.workspaces_mut().flat_map(|ws| &mut ws.tiles) {
            animations_running |= tile.advance_animations(current_time);
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
        let Some(start_data) = pointer.grab_start_data() else {
            return;
        };

        let Some(wl_surface) = window.wl_surface() else {
            return;
        };
        // Make sure we are moving the same window
        if start_data.focus.is_none()
            || !start_data
                .focus
                .as_ref()
                .unwrap()
                .0
                .same_client_as(&wl_surface.id())
        {
            return;
        }

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
}
