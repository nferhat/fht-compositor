use fht_compositor_config::DecorationMode;
use smithay::backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state};
use smithay::delegate_compositor;
use smithay::desktop::{find_popup_root_surface, PopupKind};
use smithay::output::Output;
use smithay::reexports::calloop::Interest;
use smithay::reexports::wayland_protocols::wp::content_type::v1::server::wp_content_type_v1;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::Rectangle;
use smithay::wayland::compositor::{
    add_blocker, add_pre_commit_hook, get_parent, is_sync_subsurface, remove_pre_commit_hook,
    with_states, BufferAssignment, CompositorHandler, SurfaceAttributes,
};
use smithay::wayland::content_type::ContentTypeSurfaceCachedState;
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::{SurfaceCachedState, XdgPopupSurfaceData};

use crate::state::{Fht, ResolvedWindowRules, State, UnmappedWindow};
use crate::utils::RectCenterExt;

fn has_render_buffer(surface: &WlSurface) -> bool {
    // If there's no renderer surface data, just assume the surface didn't even get recognized by
    // the renderer
    with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or_else(|| {
        warn!(
            surface = surface.id().protocol_id(),
            "Surface has no renderer state even though we use smithay buffer handler"
        );
        false
    })
}

impl State {
    fn process_window_commit(&mut self, surface: &WlSurface) -> Option<Output> {
        if let Some(idx) = self.fht.unmapped_windows.iter().position(|unmapped| {
            unmapped
                .window()
                .wl_surface()
                .is_some_and(|s| &*s == surface)
        }) {
            if !self.fht.unmapped_windows[idx].configured() {
                // We did not send an initial configure for this window yet.
                // This is the time when we send an size for the window to configure itself (and)
                //
                // This is also a good oppotunity to apply any window rules, if the user specified
                // one that matches this window. Figuring out the window size is up to the
                // workspace.
                let UnmappedWindow::Unconfigured(window) = self.fht.unmapped_windows.remove(idx)
                else {
                    unreachable!()
                };
                window.on_commit();
                window.refresh();

                let mut output = self.fht.space.active_output().clone();
                let (mut workspace_id, mut workspace_idx) = {
                    let workspace = self.fht.space.active_workspace_mut();
                    (workspace.id(), workspace.index())
                };

                // Prefer parent workspace and output when matching
                if let Some(parent_workspace) =
                    window.toplevel().parent().and_then(|parent_surface| {
                        self.fht
                            .space
                            .workspace_mut_for_window_surface(&parent_surface)
                    })
                {
                    workspace_id = parent_workspace.id();
                    workspace_idx = parent_workspace.index();
                    output = parent_workspace.output().clone();
                }

                let mut rules = ResolvedWindowRules::resolve(
                    &window,
                    &self.fht.config.rules,
                    output.name().as_str(),
                    workspace_idx,
                    false, // we are still unmapped
                );

                if let Some(named_output) = rules
                    .open_on_output
                    .as_ref()
                    .and_then(|name| self.fht.output_named(name))
                {
                    output = named_output;
                }

                let decoration_mode = rules
                    .decoration_mode
                    .unwrap_or(self.fht.config.decorations.decoration_mode);
                window.toplevel().with_pending_state(|state| {
                    // Prefer* == Set server side if client didn't specify anything.
                    // Force* == set regardless of what the client set.
                    match decoration_mode {
                        DecorationMode::ClientPreference => {
                            // Whatever the client has specified.
                        }
                        DecorationMode::PreferServerSide => {
                            let _ = state
                                .decoration_mode
                                .get_or_insert(zxdg_toplevel_decoration_v1::Mode::ServerSide);
                        }
                        DecorationMode::PreferClientSide => {
                            let _ = state
                                .decoration_mode
                                .get_or_insert(zxdg_toplevel_decoration_v1::Mode::ClientSide);
                        }
                        DecorationMode::ForceServerSide => {
                            state.decoration_mode =
                                Some(zxdg_toplevel_decoration_v1::Mode::ServerSide);
                        }
                        DecorationMode::ForceClientSide => {
                            state.decoration_mode =
                                Some(zxdg_toplevel_decoration_v1::Mode::ClientSide);
                        }
                    }
                });

                // Check whether the toplevel asked for fullscreen/maximized on creation.
                // This can override checking for other values
                let is_fullscreened = if let Some(fullscreen) = rules.fullscreen {
                    window.request_fullscreen(fullscreen);
                    fullscreen
                } else {
                    window.fullscreen()
                };

                let is_maximized = if let Some(maximized) = rules.maximized {
                    window.request_maximized(maximized);
                    maximized
                } else {
                    window.maximized()
                };

                // We have to set a floating value, no matter what.
                // - If the user asked for a floating value, use it.
                // - If the window has a parent
                // - If the window requests a size with limits (min/max)
                // - The toplevel has specified a content type
                // - Default to tiled
                let parent = window
                    .toplevel()
                    .parent()
                    .and_then(|parent_surface| self.fht.space.find_window(&parent_surface));
                let has_parent = parent.is_some();
                let (min_size, max_size) = with_states(surface, |data| {
                    let mut cached_state = data.cached_state.get::<SurfaceCachedState>();
                    let surface_data = cached_state.current();
                    (surface_data.min_size, surface_data.max_size)
                });

                // If one axis is constrained, the size is constrained.
                let width_fixed =
                    (min_size.w != 0 && max_size.w != 0) && (min_size.w == max_size.w);
                let height_fixed =
                    (min_size.h != 0 && max_size.h != 0) && (min_size.h == max_size.h);
                let has_fixed_size = width_fixed || height_fixed;

                // Games and media players get floating.
                let has_content_type = with_states(surface, |data| {
                    use wp_content_type_v1::Type;
                    let mut guard = data.cached_state.get::<ContentTypeSurfaceCachedState>();
                    let current = guard.current();
                    matches!(
                        current.content_type(),
                        Type::Photo | Type::Video | Type::Game
                    )
                });

                // If the parent is floating, the child shall be too.
                let parent_floating = parent.as_ref().is_some_and(|w| !w.tiled());

                // We only honor our floating heuristics if we dont have a fullscreen/maximized
                // state from client/rules, to avoid jankiness
                let default_floating = !(is_maximized || is_fullscreened)
                    && (has_parent || has_fixed_size || has_content_type || parent_floating);

                if let Some(floating) = rules.floating {
                    window.request_tiled(!floating);
                    window.set_rules(rules); // NOTE: apply window rules here since we need them
                                             // for the right border config to be considered
                    if !floating {
                        self.fht
                            .space
                            .prepare_unconfigured_window(&window, workspace_id);
                    }
                } else if default_floating {
                    rules.floating = Some(true);
                    if has_parent {
                        // We need to center around the parent if it exists.
                        // For example OBS child window.
                        rules.centered_in_parent = Some(true);
                    } else {
                        // Otherwise center in the workspace.
                        rules.centered = Some(true);
                    }
                    window.set_rules(rules);
                    window.request_tiled(false);
                } else {
                    window.set_rules(rules); // NOTE: apply window rules here since we need them
                                             // for the right border config to be considered
                    window.request_tiled(true);
                    self.fht
                        .space
                        .prepare_unconfigured_window(&window, workspace_id);
                }

                window.send_configure();
                self.fht.unmapped_windows.push(UnmappedWindow::Configured {
                    window,
                    workspace_id,
                });
                return Some(output);
            }

            if !has_render_buffer(surface) {
                let window = self.fht.unmapped_windows[idx].window();
                window.on_commit();
                window.refresh();
                window.send_configure();
                return None;
            }

            let UnmappedWindow::Configured {
                window,
                workspace_id,
            } = self.fht.unmapped_windows.remove(idx)
            else {
                unreachable!("Tried to map an unconfigured window!");
            };

            self.fht.adversite_new_foreign_window(&window);
            window.on_commit();
            window.refresh();

            // NOTE: The pre-commit-hook assumes we only add it when we are about to map the window.
            // we also remove it when unmapping.
            super::xdg_shell::add_window_pre_commit_hook(&window);

            let workspace = match self.fht.space.workspace_mut_for_id(workspace_id) {
                Some(ws) => ws,
                None => {
                    warn!(?workspace_id, "Unmapped window has an invalid workspace id");
                    self.fht.space.active_workspace_mut()
                }
            };

            let output = workspace.output().clone();
            workspace.insert_window(window.clone(), true);
            let window_geometry = Rectangle::new(
                self.fht.space.window_location(&window).unwrap(),
                window.size(),
            );

            let is_active = self.fht.space.active_workspace_id() == workspace_id;
            let should_focus = self.fht.config.general.focus_new_windows && is_active;

            if should_focus {
                let center = window_geometry.center();
                self.fht.loop_handle.insert_idle(move |state| {
                    if state.fht.config.general.cursor_warps {
                        state.move_pointer(center.to_f64());
                    }
                    state.set_keyboard_focus(Some(window));
                });
            }

            return Some(output);
        }

        // Other check: its a mapped window.
        if let Some((window, workspace)) = self.fht.space.find_window_and_workspace_mut(surface) {
            window.on_commit();
            let is_mapped = has_render_buffer(surface);
            #[allow(unused_assignments)]
            if !is_mapped {
                // workspace.close_window will remove the window from the workspace tiles and
                // create a ClosingTile to represent the last frame of the closing window.
                self.backend.with_renderer(|renderer| {
                    if workspace.prepare_close_animation_for_window(&window, renderer) {
                        workspace.close_window(&window, renderer, true);
                    }
                });

                if let Some(pre_commit_hook) = window.take_pre_commit_hook_id() {
                    remove_pre_commit_hook(surface, pre_commit_hook);
                }

                // When a window gets unmapped, it needs to go through all the initial configure
                // sequence again to set its render buffers and toplevel surface again.
                let output = workspace.output().clone();
                self.fht
                    .unmapped_windows
                    .push(UnmappedWindow::Unconfigured(window));
                return Some(output);
            }
            return Some(workspace.output().clone());
        }

        None
    }

    fn process_popup_commit(surface: &WlSurface, state: &mut Fht) -> Option<Output> {
        let popup = state.popups.find_popup(surface)?;

        match popup {
            PopupKind::Xdg(ref popup) => {
                let initial_configure_sent = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<XdgPopupSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .initial_configure_sent
                });
                if !initial_configure_sent {
                    // NOTE: This should never fail as the initial configure is always
                    // allowed.
                    popup.send_configure().expect("initial configure failed");
                }
            }
            PopupKind::InputMethod(_) => {
                // Input method popups dont need an initial configure.
            }
        }

        let root = find_popup_root_surface(&popup).ok()?;
        state.visible_output_for_surface(&root).cloned()
    }
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut smithay::wayland::compositor::CompositorState {
        &mut self.fht.compositor_state
    }

    fn client_compositor_state<'a>(
        &self,
        client: &'a smithay::reexports::wayland_server::Client,
    ) -> &'a smithay::wayland::compositor::CompositorClientState {
        &client
            .get_data::<crate::state::ClientState>()
            .unwrap()
            .compositor
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<Self, _>(surface, move |state, _dh, surface| {
            let maybe_dmabuf = with_states(surface, |surface_data| {
                surface_data
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .pending()
                    .buffer
                    .as_ref()
                    .and_then(|assignment| match assignment {
                        BufferAssignment::NewBuffer(buffer) => get_dmabuf(&buffer).cloned().ok(),
                        _ => None,
                    })
            });
            if let Some(dmabuf) = maybe_dmabuf {
                if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ) {
                    let client = surface.client().unwrap();
                    let res = state
                        .fht
                        .loop_handle
                        .insert_source(source, move |_, _, state| {
                            let dh = state.fht.display_handle.clone();
                            state
                                .client_compositor_state(&client)
                                .blocker_cleared(state, &dh);
                            Ok(())
                        });
                    if res.is_ok() {
                        add_blocker(surface, blocker);
                    }
                }
            }
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        crate::profile_function!();
        // Allow smithay to manage internally wl_surfaces and wl_buffers
        //
        // Have to call this at the top of here before handling anything otherwise it'll mess
        // buffer management
        on_commit_buffer_handler::<Self>(surface);
        #[cfg(feature = "udev-backend")]
        #[allow(irrefutable_let_patterns)]
        if let crate::backend::Backend::Udev(ref mut data) = &mut self.backend {
            data.early_import(surface);
        }

        // We are already synced, why bother going additional computations
        if is_sync_subsurface(surface) {
            return;
        }

        let mut root_surface = surface.clone();
        while let Some(new_parent) = get_parent(&root_surface) {
            root_surface = new_parent;
        }
        // cache our root surface, see [`CompositorHandler::destroyed`]
        self.fht
            .root_surfaces
            .insert(surface.clone(), root_surface.clone());

        if surface == &root_surface {
            // Committing a root surface, not a subsurface/popup.
            // Try to get the output where this surface is being drawn, otherwise quit.
            if let Some(output) = self
                .process_window_commit(surface)
                .or_else(|| State::process_layer_shell_commit(&surface, &mut self.fht))
            {
                self.fht.queue_redraw(&output);
            }
        }

        // 1st case if this isnt a root surface; a popup.
        // Ensure initial configure.
        self.fht.popups.commit(surface);
        if let Some(output) = State::process_popup_commit(surface, &mut self.fht) {
            self.fht.queue_redraw(&output);
            return;
        }

        // 2nd case if this isnt a root surface; some kind of subsurface.
        // For example firefox has its main webcontent as a subsurface.
        if let Some(output) = self.fht.visible_output_for_surface(surface).cloned() {
            self.fht.queue_redraw(&output);
        }
    }

    fn destroyed(&mut self, surface: &WlSurface) {
        // Some clients may destroy their subsurfaces before their main surface. If they do, some
        // internal handling in smithay causes the following:
        // - `get_parent()` is useless
        // - the surface render state view is reset.
        //
        // We want the closing animation to include *all* subsurfaces of our window.
        //
        // As niri states it, this is not perfect, but still better than nothing.
        if let Some(root) = self.fht.root_surfaces.get(surface).cloned() {
            if let Some((window, workspace)) = self.fht.space.find_window_and_workspace_mut(&root) {
                self.backend.with_renderer(|renderer| {
                    workspace.prepare_close_animation_for_window(&window, renderer);
                });
            }
        }

        self.fht
            .root_surfaces
            .retain(|k, v| k != surface && v != surface)
    }
}

delegate_compositor!(State);
