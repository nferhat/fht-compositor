use fht_compositor_config::DecorationMode;
use smithay::delegate_xdg_shell;
use smithay::desktop::{
    find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output, LayerSurface,
    PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, WindowSurfaceType,
};
use smithay::input::pointer::{CursorIcon, CursorImageStatus, Focus};
use smithay::input::Seat;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::content_type::v1::server::wp_content_type_v1;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::{
    self, State as ToplevelState, WmCapabilities,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::protocol::{wl_output, wl_seat};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point, Rectangle, Serial};
use smithay::wayland::compositor::{
    add_pre_commit_hook, with_states, BufferAssignment, SurfaceAttributes,
};
use smithay::wayland::content_type::ContentTypeSurfaceCachedState;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellHandler,
    XdgShellState,
};

use crate::focus::KeyboardFocus;
use crate::input::resize_tile_grab::{ResizeEdge, ResizeTileGrab};
use crate::input::swap_tile_grab::SwapTileGrab;
use crate::output::OutputExt;
use crate::space::{Workspace, WorkspaceId};
use crate::state::{Fht, ResolvedWindowRules, State, UnmappedWindow};
use crate::utils::RectCenterExt as _;
use crate::window::Window;

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.fht.xdg_shell_state
    }

    fn new_toplevel(&mut self, toplevel: ToplevelSurface) {
        let surface = toplevel.wl_surface().clone();
        let window = Window::new(toplevel);
        if let Some(_) = self
            .fht
            .unmapped_windows
            .insert(surface.clone(), UnmappedWindow::Unconfigured(window))
        {
            warn!(id = %surface.id(), "Surface opened toplevel twice");
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(_) = self.fht.unmapped_windows.remove(surface.wl_surface()) {
            // Well, it was never mapped, so nothing changes I guess.
            return;
        }

        let Some((window, workspace)) = self
            .fht
            .space
            .find_window_and_workspace_mut(surface.wl_surface())
        else {
            warn!("Destroyed toplevel missing from mapped windows and unmapped windows");
            return;
        };

        self.backend.with_renderer(|renderer| {
            if workspace.prepare_close_animation_for_window(&window, renderer) {
                workspace.close_window(&window, renderer, true);
            }
        });

        let output = workspace.output().clone();
        self.fht.queue_redraw(&output);

        // dont forget to remove the foreign toplevel handle.
        //
        // NOTE: I am not sure but this should always be emitted, regardless of whether we or the
        // toplevel closes (since we use send_close request)
        self.fht.close_foreign_handle(&window);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.fht.unconstrain_popup(&surface);
        if let Err(err) = self.fht.popups.track_popup(PopupKind::from(surface)) {
            warn!(?err, "Failed to track popup!")
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, _: wl_seat::WlSeat, serial: Serial) {
        let pointer = self.fht.pointer.clone();
        let mut grab_start_data = None;

        pointer.with_grab(|grab_serial, grab| {
            if grab_serial == serial {
                let start_data = grab.start_data();
                if start_data
                    .focus
                    .as_ref()
                    .is_some_and(|(focus, _)| focus.same_client_as(&surface.wl_surface().id()))
                {
                    grab_start_data = Some(start_data.clone());
                }
            }
        });

        let Some(start_data) = grab_start_data else {
            return;
        };

        if let Some(window) = self.fht.space.find_window(surface.wl_surface()) {
            if self.fht.space.start_interactive_swap(
                &window,
                start_data.location.to_i32_round(),
                // Disable move window since when a move_request happens, its most likely that you
                // grabbed the window from a titlebar, so keep the cursor there.
                false,
            ) {
                let grab = SwapTileGrab { window, start_data };
                pointer.set_grab(self, grab, serial, Focus::Clear);
                self.fht
                    .cursor_theme_manager
                    .set_image_status(CursorImageStatus::Named(CursorIcon::Grabbing));
            }
        }
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let pointer = self.fht.pointer.clone();
        let mut grab_start_data = None;

        pointer.with_grab(|grab_serial, grab| {
            if grab_serial == serial {
                let start_data = grab.start_data();
                if start_data
                    .focus
                    .as_ref()
                    .is_some_and(|(focus, _)| focus.same_client_as(&surface.wl_surface().id()))
                {
                    grab_start_data = Some(start_data.clone());
                }
            }
        });

        let Some(start_data) = grab_start_data else {
            return;
        };

        let mut output = None;
        if let Some((window, workspace)) = self
            .fht
            .space
            .find_window_and_workspace_mut(surface.wl_surface())
        {
            let edges = ResizeEdge::from(edges);
            if workspace.start_interactive_resize(&window, edges) {
                let ws_output = workspace.output().clone();
                output = Some(ws_output.clone()); // augh, the borrow checker
                let grab = ResizeTileGrab {
                    window,
                    output: ws_output,
                    start_data,
                };
                pointer.set_grab(self, grab, serial, Focus::Clear);
            }
        }

        if let Some(ref output) = output {
            self.fht.queue_redraw(output);
        }
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat: Seat<State> = Seat::from_resource(&seat).unwrap();
        let popup_kind = PopupKind::Xdg(surface);

        if let Some(root) = find_popup_root_surface(&popup_kind).ok().and_then(|root| {
            self.fht
                .space
                .find_window(&root)
                .map(|win| KeyboardFocus::Space {
                    surface: Some(win.wl_surface().clone()),
                })
                .or_else(|| {
                    self.fht
                        .space
                        .outputs()
                        .find_map(|o| {
                            layer_map_for_output(o)
                                .layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)
                                .cloned()
                        })
                        .map(|layer| KeyboardFocus::LayerSurface { layer })
                })
        }) {
            let wl_surface = root.wl_surface().unwrap().into_owned();
            let grab = self
                .fht
                .popups
                .grab_popup(wl_surface.clone(), popup_kind, &seat, serial);

            if let Ok(mut grab) = grab {
                if let Some(keyboard) = seat.get_keyboard() {
                    if keyboard.is_grabbed()
                        && !(keyboard.has_grab(serial)
                            || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
                    {
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }
                    keyboard.set_focus(self, grab.current_grab(), serial);
                    keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
                }
                if let Some(pointer) = seat.get_pointer() {
                    if pointer.is_grabbed()
                        && !(pointer.has_grab(serial)
                            || pointer
                                .has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
                    {
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }
                    pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
                }
            }
        }
    }

    fn maximize_request(&mut self, toplevel: ToplevelSurface) {
        let can_maximize = toplevel.with_committed_state(|state| {
            state.map_or(false, |state| {
                state.capabilities.contains(WmCapabilities::Maximize)
            })
        });
        if can_maximize {
            let wl_surface = toplevel.wl_surface();
            if let Some(window) = self.fht.space.find_window(wl_surface) {
                if self.fht.space.maximize_window(
                    &window,
                    true,
                    !self.fht.config.animations.disable,
                ) {
                    window.request_maximized(true);
                }
            }
        }

        toplevel.send_configure();
    }

    fn unmaximize_request(&mut self, toplevel: ToplevelSurface) {
        if let Some((window, ws)) = self
            .fht
            .space
            .find_window_and_workspace_mut(toplevel.wl_surface())
        {
            window.request_maximized(false);
            ws.arrange_tiles(true);
        }

        toplevel.send_configure();
    }

    fn fullscreen_request(
        &mut self,
        toplevel: ToplevelSurface,
        wl_output: Option<wl_output::WlOutput>,
    ) {
        let can_fullscreen = toplevel.with_committed_state(|state| {
            state.map_or(false, |state| {
                state.capabilities.contains(WmCapabilities::Fullscreen)
            })
        });

        if can_fullscreen {
            let wl_surface = toplevel.wl_surface();
            if let Some(window) = self.fht.space.find_window(wl_surface) {
                if let Some(requested) = wl_output.as_ref().and_then(Output::from_resource) {
                    self.fht
                        .space
                        .move_window_to_output(&window, &requested, true);
                }

                window.request_fullscreen(true);
                if !self.fht.space.fullscreen_window(&window, true) {
                    window.request_fullscreen(false);
                }
            }
        }

        toplevel.send_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.fht.space.find_window(surface.wl_surface()) {
            // NOTE: Workspaces take care of unfullscreening and arranging
            window.request_fullscreen(false);
        }

        surface.send_configure();
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.fht.space.find_window(surface.wl_surface()) {
            self.fht.send_foreign_window_details(&window);
            self.fht.resolve_rules_for_window(&window);
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.fht.space.find_window(surface.wl_surface()) {
            self.fht.send_foreign_window_details(&window);
            self.fht.resolve_rules_for_window(&window);
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        self.fht.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }
}

pub(super) fn add_window_pre_commit_hook(window: &Window) {
    // The workspace tile api is not responsible for actually starting the close animations, we are
    // the ones that should do this.
    let wl_surface = window.wl_surface();
    let hook_id = add_pre_commit_hook::<State, _>(&wl_surface, |state, _dh, surface| {
        if let Some((window, workspace)) = state.fht.space.find_window_and_workspace_mut(surface) {
            // Before commiting, we check if the window's buffers are getting unmapped.
            // If that's the case, the window is likely closing (or minimizing, if the
            // compositor supports that)
            //
            // Since we are going to close, we take a snapshot of the window's elements,
            // like we do inside `Tile::render_elements` into a
            // GlesTexture and store that for future use.
            let got_unmapped = with_states(surface, |states| {
                let mut guard = states.cached_state.get::<SurfaceAttributes>();
                let attrs = guard.pending();
                matches!(attrs.buffer, Some(BufferAssignment::Removed))
            });

            if got_unmapped {
                state.backend.with_renderer(|renderer| {
                    workspace.prepare_close_animation_for_window(&window, renderer);
                });
            } else {
                workspace.clear_close_animation_for_window(&window);
            }
        };

        if let Some((grabbed_tile, output)) = state.fht.space.interactive_swap_tile_mut() {
            // Same logic but for interactive swap. Nothing special.
            let got_unmapped = with_states(surface, |states| {
                let mut guard = states.cached_state.get::<SurfaceAttributes>();
                let attrs = guard.pending();
                matches!(attrs.buffer, Some(BufferAssignment::Removed))
            });

            if got_unmapped {
                let scale = output
                    .map(|o| o.current_scale().integer_scale())
                    .unwrap_or(1);
                state.backend.with_renderer(|renderer| {
                    grabbed_tile.prepare_close_animation_if_needed(renderer, scale);
                });
            } else {
                grabbed_tile.clear_close_animation_snapshot();
            }
        }
    });

    window.set_pre_commit_hook_id(hook_id);
}

impl Fht {
    pub fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };

        if let Some((window, workspace)) = self.space.find_window_and_workspace(&root) {
            self.unconstrain_window_popup(popup, window, workspace);
        } else if let Some((layer_surface, output)) = self.space.outputs().find_map(|o| {
            let layer_map = layer_map_for_output(o);
            let layer_surface = layer_map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)?;
            Some((layer_surface.clone(), o.clone()))
        }) {
            self.unconstrain_layer_popup(popup, &layer_surface, &output);
        };
    }

    pub fn unconstrain_window_popup(
        &self,
        popup: &PopupSurface,
        window: Window,
        workspace: &Workspace,
    ) {
        // we constrain the popup inside the output the window is, to avoid overflows
        let mut target = Rectangle::from_size(workspace.output().geometry().size);
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= workspace.window_location(&window).unwrap();

        self.place_popup_inside(popup, target);
    }

    pub fn unconstrain_layer_popup(
        &self,
        popup: &PopupSurface,
        layer_surface: &LayerSurface,
        output: &Output,
    ) {
        let output_geo = output.geometry();
        let map = layer_map_for_output(output);
        let Some(layer_geo) = map.layer_geometry(layer_surface) else {
            return;
        };

        // The target geometry for the positioner should be relative to its parent's geometry, so
        // we will compute that here.
        let mut target = Rectangle::from_size(output_geo.size);
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= layer_geo.loc;

        self.place_popup_inside(popup, target);
    }

    pub fn place_popup_inside(&self, popup: &PopupSurface, target: Rectangle<i32, Logical>) {
        popup.with_pending_state(|state| {
            // We try to unconstrain with some padding, but, we can do without
            const PADDING: i32 = 10;
            let mut padded = target;
            if PADDING * 2 < padded.size.w {
                padded.loc.x += PADDING;
                padded.size.w -= PADDING * 2;
            }
            if PADDING * 2 < padded.size.h {
                padded.loc.y += PADDING;
                padded.size.h -= PADDING * 2;
            }

            if padded == target {
                // We couldn't add padding, so just unconstrain as usual
                state.geometry = state.positioner.get_unconstrained_geometry(target);
                return;
            }

            // Do not try to resize to fit the padded target rectangle.
            let mut no_resize = state.positioner;
            no_resize
                .constraint_adjustment
                .remove(ConstraintAdjustment::ResizeX);
            no_resize
                .constraint_adjustment
                .remove(ConstraintAdjustment::ResizeY);

            let geo = no_resize.get_unconstrained_geometry(padded);
            if padded.contains_rect(geo) {
                state.geometry = geo;
                return;
            }

            // Could not unconstrain into the padded target, so resort to the regular one.
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    pub fn queue_initial_configure(&self, surface: WlSurface, window: Window) {
        self.loop_handle.insert_idle(move |state| {
            state.fht.send_initial_configure(surface, window);
        });
    }

    fn send_initial_configure(&mut self, surface: WlSurface, window: Window) {
        let window_id = window.id();
        trace!(?window_id, "Preparing unconfigured window");
        window.on_commit();
        window.refresh();

        // In order to calculate window rules, we must know the current output/workspace.
        // We have on-output/workspace matches, so they shall be respected
        let (has_parent, current_output, current_ws_idx, current_ws_id) =
            if let Some(parent_workspace) = window
                .toplevel()
                .parent()
                .and_then(|parent| self.space.workspace_mut_for_window_surface(&parent))
            {
                trace!(id = ?parent_workspace.id(), "found parent mapped in workspace");
                (
                    true,
                    parent_workspace.output().clone(),
                    parent_workspace.index(),
                    parent_workspace.id(),
                )
            } else {
                (
                    false,
                    self.space.active_output().clone(),
                    self.space.active_workspace_mut().index(),
                    self.space.active_workspace_mut().id(),
                )
            };

        let mut rules = ResolvedWindowRules::resolve(
            &window,
            &self.config.rules,
            &current_output.name(),
            current_ws_idx,
            false, // we are still unmapped
        );

        let opening_location = rules.location;

        let open_on_output = if let Some(named_output) = rules
            .open_on_output
            .as_ref()
            .and_then(|name| self.output_named(name))
        {
            named_output
        } else {
            current_output
        };

        let open_on_workspace = if let Some(open_on_workspace) = rules.open_on_workspace {
            let mon = self.space.monitor_mut_for_output(&open_on_output).unwrap();
            mon.workspace_by_index(open_on_workspace.clamp(0, 8)).id()
        } else {
            current_ws_id
        };

        let decoration_mode = match rules
            .decoration_mode
            .unwrap_or(self.config.decorations.decoration_mode)
        {
            // The decoration mode to apply.
            // prefer-* branches will just keep whatever the client has.
            DecorationMode::ClientPreference
            | DecorationMode::PreferClientSide
            | DecorationMode::PreferServerSide => None,
            DecorationMode::ForceClientSide => Some(zxdg_toplevel_decoration_v1::Mode::ClientSide),
            DecorationMode::ForceServerSide => Some(zxdg_toplevel_decoration_v1::Mode::ServerSide),
        };

        let open_floating = if let Some(open_floating) = rules.floating {
            open_floating
        } else {
            should_open_window_floating(&window)
        };

        window.toplevel().with_pending_state(|pending| {
            pending.decoration_mode = decoration_mode;
            if let Some(fullscreen) = rules.fullscreen {
                if fullscreen {
                    pending.states.set(ToplevelState::Fullscreen);
                } else {
                    pending.states.unset(ToplevelState::Fullscreen);
                }
            }

            if let Some(maximized) = rules.maximized {
                if maximized {
                    pending.states.set(ToplevelState::Maximized);
                } else {
                    pending.states.unset(ToplevelState::Maximized);
                }
            }

            if !open_floating {
                pending.states.set(ToplevelState::TiledBottom);
                pending.states.set(ToplevelState::TiledLeft);
                pending.states.set(ToplevelState::TiledRight);
                pending.states.set(ToplevelState::TiledTop);
            } else {
                pending.states.unset(ToplevelState::TiledBottom);
                pending.states.unset(ToplevelState::TiledLeft);
                pending.states.unset(ToplevelState::TiledRight);
                pending.states.unset(ToplevelState::TiledTop);
            }
        });

        if open_floating {
            if has_parent {
                rules.centered_in_parent = Some(true);
            } else {
                // FIXME: Perhaps calculate "the best place to put this floating window" in the
                // workspace? Because right how you can spam open window and they will all end up
                // in the same place with this.
                rules.centered = Some(true);
            }

            window.set_rules(rules);
        } else {
            window.set_rules(rules);
            self.space
                .prepare_unconfigured_window(&window, open_on_workspace);
        }

        // Now force-send a configure to send the initial configure message.
        window.send_configure();
        self.unmapped_windows.insert(
            surface,
            UnmappedWindow::Configured {
                window,
                workspace_id: open_on_workspace,
                opening_location,
            },
        );
    }

    /// Maps the given [`Window`]. Returns the [`Output`] on which it gets mapped.
    pub fn map_window(
        &mut self,
        window: Window,
        workspace_id: WorkspaceId,
        opening_location: Option<Point<i32, Logical>>,
    ) -> Output {
        // Do another check again just in-case the window decided to change
        // its mind for absolutely no reason. (which happens)
        let is_floating = !window.tiled();
        let opening_location = opening_location.filter(|_| is_floating);

        self.advertise_new_foreign_window(&window);
        window.on_commit();
        window.refresh();

        // NOTE: The pre-commit-hook assumes we only add it when we are about to map the window.
        // we also remove it when unmapping.
        super::xdg_shell::add_window_pre_commit_hook(&window);

        let workspace = match self.space.workspace_mut_for_id(workspace_id) {
            Some(ws) => ws,
            None => {
                warn!(?workspace_id, "Unmapped window has an invalid workspace id");
                self.space.active_workspace_mut()
            }
        };

        let output = workspace.output().clone();
        workspace.insert_window(window.clone(), opening_location, true);
        let window_geometry =
            Rectangle::new(self.space.window_location(&window).unwrap(), window.size());

        let is_active = self.space.active_workspace_id() == workspace_id;
        let should_focus = self.config.general.focus_new_windows && is_active;

        if should_focus {
            let center = window_geometry.center();
            self.loop_handle.insert_idle(move |state| {
                if state.fht.config.general.cursor_warps {
                    state.move_pointer(center.to_f64());
                }
                state.set_keyboard_focus(Some(window.wl_surface().clone()));
            });
        }

        output
    }
}

/// Determine whether a window should be opened floating based on a set of heuristics.
/// These are from cage, sway, hyprland and niri.
///
/// These of course don't cover all the cases, use window rules if they don't cover yours.
///
/// The current list of heuristics include
/// - Window has a parent
/// - Window requests a size with limits (min/max)
/// - Window has a content type
/// - Window is a modal/dialog
fn should_open_window_floating(window: &Window) -> bool {
    // Window with parents should always open floating. If you don't like it just use
    // a window rule, simple
    if window.toplevel().parent().is_some() {
        trace!("floating window with parent");
        return true;
    }

    // Fixed-size clients should be floating too.
    let wl_surface = window.wl_surface();
    let (min_size, max_size) = with_states(&wl_surface, |data| {
        let mut cached_state = data.cached_state.get::<SurfaceCachedState>();
        let surface_data = cached_state.current();
        (surface_data.min_size, surface_data.max_size)
    });

    if min_size.h > 0 && min_size.h == max_size.h {
        trace!("floating window due to matching fixed-height");
        // Heuristics from sway.
        return true;
    }

    // Games and media players get floating.
    // FIXME: Make this a window rule
    let has_content_type = with_states(&*wl_surface, |data| {
        use wp_content_type_v1::Type;
        let mut guard = data.cached_state.get::<ContentTypeSurfaceCachedState>();
        let current = guard.current();
        matches!(
            current.content_type(),
            Type::Photo | Type::Video | Type::Game
        )
    });

    if has_content_type {
        trace!("floating window due to matching content-type");
        return true;
    }

    false
}

delegate_xdg_shell!(State);
