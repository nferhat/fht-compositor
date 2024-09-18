use smithay::backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state};
use smithay::delegate_compositor;
use smithay::desktop::{find_popup_root_surface, PopupKind};
use smithay::output::Output;
use smithay::reexports::calloop::Interest;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::{
    add_blocker, add_pre_commit_hook, get_parent, is_sync_subsurface, with_states,
    BufferAssignment, CompositorHandler, SurfaceAttributes,
};
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::XdgPopupSurfaceData;

use crate::config::CONFIG;
use crate::state::{Fht, OutputState, State, UnmappedWindow};
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

                let mut output = self.fht.focus_state.output.clone().unwrap();
                let mut workspace_idx = self.fht.wset_for(&output).get_active_idx();

                // Prefer parent workspace and output when matching
                if let Some((parent_index, parent_output)) =
                    window.toplevel().parent().and_then(|parent_surface| {
                        self.fht.workspaces().find_map(|(output, wset)| {
                            let idx = wset
                                .workspaces()
                                .enumerate()
                                .position(|(_, ws)| ws.has_surface(&parent_surface))?;
                            Some((idx, output.clone()))
                        })
                    })
                {
                    workspace_idx = parent_index;
                    output = parent_output;
                }

                let (title, app_id) = (window.title(), window.app_id());
                let rule = CONFIG
                    .rules
                    .iter()
                    .find(|(patterns, _)| {
                        patterns.iter().any(|pattern| {
                            pattern.matches(
                                title.as_ref().map(String::as_str),
                                app_id.as_ref().map(String::as_str),
                                workspace_idx,
                            )
                        })
                    })
                    .map(|(_, rules)| rules)
                    .cloned()
                    .unwrap_or_default();

                if let Some(named_output) = rule
                    .output
                    .as_ref()
                    .and_then(|name| self.fht.output_named(name))
                {
                    output = named_output;
                }

                if let Some(allow_csd) = rule.allow_csd {
                    window.toplevel().with_pending_state(|state| {
                        if allow_csd {
                            state.decoration_mode =
                                Some(zxdg_toplevel_decoration_v1::Mode::ClientSide);
                        } else {
                            state.decoration_mode =
                                Some(zxdg_toplevel_decoration_v1::Mode::ServerSide);
                        }
                    });
                } else {
                    window.toplevel().with_pending_state(|state| {
                        if CONFIG.decoration.allow_csd {
                            state.decoration_mode =
                                Some(zxdg_toplevel_decoration_v1::Mode::ClientSide);
                        } else {
                            state.decoration_mode =
                                Some(zxdg_toplevel_decoration_v1::Mode::ServerSide);
                        }
                    });
                }

                // Now apply our rules
                let wset = self.fht.wset_mut_for(&output);
                let workspace_idx = rule.workspace.unwrap_or(wset.get_active_idx());
                let workspace = wset.get_workspace_mut(workspace_idx);
                let id = workspace.id();

                // Pre compute window geometry for insertion.
                workspace.prepare_window_geometry(window.clone(), rule.border.clone());
                window.send_configure();
                self.fht.unmapped_windows.push(UnmappedWindow::Configured {
                    window,
                    border_config: rule.border,
                    workspace_id: id,
                });
                return Some(output);
            }

            if !has_render_buffer(surface) {
                self.fht.unmapped_windows[idx].window().send_configure();
                return None;
            }

            let UnmappedWindow::Configured {
                window,
                border_config,
                workspace_id,
            } = self.fht.unmapped_windows.remove(idx)
            else {
                unreachable!("Tried to map an unconfigured window!");
            };

            let workspace = match self.fht.get_workspace_mut(workspace_id) {
                Some(ws) => ws,
                None => {
                    warn!(
                        ?workspace_id,
                        "Unmapped window has an invalid workspace id?"
                    );
                    let output = self.fht.active_output();
                    self.fht.wset_mut_for(&output).active_mut()
                }
            };

            let output = workspace.output();
            workspace.insert_window(window.clone(), border_config, true);
            let mut geometry = workspace.window_geometry(&window).unwrap();
            geometry.loc += output.current_location();

            let wset = self.fht.wset_for(&output);
            let is_active = wset.active().id() == workspace_id;
            let is_switching = wset.has_switch_animation();
            // From using the compositor opening a window when a switch is being done feels more
            // natural when the window gets focus, even if focus_new_windows is none.
            let should_focus = (CONFIG.general.focus_new_windows || is_switching) && is_active;

            if should_focus {
                let center = geometry.center();
                self.fht.loop_handle.insert_idle(move |state| {
                    if CONFIG.general.cursor_warps {
                        state.move_pointer(center.to_f64());
                    }
                    state.set_focus_target(Some(window.clone().into()));
                });
            }

            return Some(output);
        }

        // Other check: its a mapped window.
        let arrange = false;
        if let Some((window, output)) = self.fht.find_window_and_output(surface) {
            let is_mapped = has_render_buffer(surface);
            #[allow(unused_assignments)]
            // TODO:
            // if !is_mapped {
            //     // The window's render surface got removed, start out close animation.
            //     // The unmap snapshot you have be prepared earlier, either by:
            //     //
            //     // - XdgShellHandler::toplevel_destroyed
            //     // - the pre-commit hook we set up on XdgShellHandler::new_toplevel
            //     self.backend.with_renderer(|renderer| {
            //         let scale = output.current_scale().fractional_scale().into();
            //         tile.start_close_animation(renderer, scale);
            //     });
            //     arrange = true;
            // }
            window.on_commit();
            return Some(output.clone());
        }

        if arrange {
            let (_, ws) = self.fht.find_window_and_workspace_mut(surface).unwrap();
            ws.arrange_tiles(true);
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

    #[profiling::function]
    fn commit(&mut self, surface: &WlSurface) {
        // Allow smithay to manage internally wl_surfaces and wl_buffers
        //
        // Have to call this at the top of here before handling anything otherwise it'll mess
        // buffer management
        on_commit_buffer_handler::<Self>(surface);
        #[cfg(feature = "udev_backend")]
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
                OutputState::get(&output).render_state.queue();
            }
        }

        // 1st case if this isnt a root surface; a popup.
        // Ensure initial configure.
        self.fht.popups.commit(surface);
        if let Some(output) = State::process_popup_commit(surface, &mut self.fht) {
            OutputState::get(&output).render_state.queue();
            return;
        }

        // 2nd case if this isnt a root surface; some kind of subsurface.
        // For example firefox has its main webcontent as a subsurface.
        if let Some(output) = self.fht.visible_output_for_surface(surface) {
            OutputState::get(&output).render_state.queue();
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
            // TODO:
            // if let Some((tile, output)) = self.fht.find_tile_and_output(&root) {
            //     self.backend.with_renderer(|renderer| {
            //         let scale = output.current_scale().fractional_scale().into();
            //         tile.prepare_close_animation(renderer, scale);
            //     });
            // }
        }

        self.fht
            .root_surfaces
            .retain(|k, v| k != surface && v != surface)
    }
}

delegate_compositor!(State);
