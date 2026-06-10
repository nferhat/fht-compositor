use std::collections::hash_map::Entry;

use smithay::backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state};
use smithay::delegate_compositor;
use smithay::desktop::PopupKind;
use smithay::reexports::calloop::Interest;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::{
    add_blocker, add_pre_commit_hook, get_parent, is_sync_subsurface, remove_pre_commit_hook,
    with_states, BufferAssignment, CompositorHandler, SurfaceAttributes,
};
use smithay::wayland::dmabuf::get_dmabuf;

use crate::state::{State, UnmappedWindow};
use crate::utils::send_scale_transform;

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
                        BufferAssignment::NewBuffer(buffer) => get_dmabuf(buffer).cloned().ok(),
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

        // cache our root surface, see [`CompositorHandler::destroyed`]
        let mut root_surface = surface.clone();
        while let Some(new_parent) = get_parent(&root_surface) {
            root_surface = new_parent;
        }
        self.fht
            .root_surfaces
            .insert(surface.clone(), root_surface.clone());

        // We are already synced, why bother going additional computations
        if is_sync_subsurface(surface) {
            return;
        }

        if surface == &root_surface {
            // Maybe it's an unmapped window.
            if let Entry::Occupied(entry) = self.fht.unmapped_windows.entry(surface.clone()) {
                if matches!(entry.get(), UnmappedWindow::Unconfigured(_)) {
                    let UnmappedWindow::Unconfigured(window) = entry.remove() else {
                        unreachable!()
                    };
                    self.fht.queue_initial_configure(surface.clone(), window);
                    return; // nothing happening yet!
                } else {
                    if !has_render_buffer(surface) {
                        let window = entry.get().window();
                        window.on_commit();
                        window.refresh();
                        window.send_configure();
                        return;
                    }

                    let UnmappedWindow::Configured {
                        window,
                        workspace_id,
                        opening_location,
                    } = entry.remove()
                    else {
                        unreachable!()
                    };

                    let output = self.fht.map_window(window, workspace_id, opening_location);
                    self.fht.queue_redraw(&output);
                    return;
                }
            }

            // Maybe it's a mapped window.
            if let Some((window, workspace)) = self.fht.space.find_window_and_workspace_mut(surface)
            {
                let is_mapped = has_render_buffer(surface);
                let output = workspace.output().clone();

                if !is_mapped {
                    // workspace.close_window will remove the window from the workspace tiles and
                    // create a ClosingTile to represent the last frame of the closing window.
                    self.backend.with_renderer(|renderer| {
                        if workspace.prepare_close_animation_for_window(&window, renderer) {
                            workspace.close_window(&window, renderer, true);
                        }
                    });
                }

                window.on_commit();

                if !is_mapped {
                    if let Some(pre_commit_hook) = window.take_pre_commit_hook_id() {
                        remove_pre_commit_hook(surface, &pre_commit_hook);
                    }

                    // When a window gets unmapped, it needs to go through all the initial configure
                    // sequence again to set its render buffers and toplevel surface again.
                    self.fht
                        .unmapped_windows
                        .insert(surface.clone(), UnmappedWindow::Unconfigured(window));

                    self.fht.queue_redraw(&output);
                    return;
                }

                self.fht.queue_redraw(&output);
                return;
            }
        }

        // This is the commit of a non-root wl surface.
        // we still need to update/commit the root surfce
        if let Some((window, ws)) = self.fht.space.find_window_and_workspace_mut(&root_surface) {
            let output = ws.output().clone();
            // FIXME: We should probably tell the workspace about the window update here, but we
            // instead wait until the next refresh. (IE next State::dispatch)
            window.on_commit();
            self.fht.queue_redraw(&output);
            return;
        }

        // This could be a popup
        {
            self.fht.popups.commit(surface);
            if let Some(popup) = self.fht.popups.find_popup(surface) {
                match popup {
                    PopupKind::Xdg(ref popup) => {
                        if !popup.is_initial_configure_sent() {
                            if let Some(output) =
                                self.fht.output_for_popup(&PopupKind::Xdg(popup.clone()))
                            {
                                let scale = output.current_scale();
                                let transform = output.current_transform();
                                with_states(surface, |data| {
                                    send_scale_transform(surface, data, scale, transform);
                                });
                            }
                            popup
                                .send_configure()
                                .expect("popup initial configure failed");
                        }
                    }
                    // IME popups don't need a configure.
                    PopupKind::InputMethod(_) => {}
                }
            }

            if let Some(popup) = self.fht.popups.find_popup(surface) {
                if let Some(output) = self.fht.output_for_popup(&popup) {
                    self.fht.queue_redraw(&output.clone());
                }
                return;
            }
        }

        // This could be a layer-shell.
        if let Some(output) = State::process_layer_shell_commit(surface, &mut self.fht) {
            self.fht.queue_redraw(&output);
            return;
        }

        trace!(id = %surface.id(), "unknown surface commit");
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
