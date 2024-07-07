use smithay::backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state};
use smithay::delegate_compositor;
use smithay::desktop::space::SpaceElement;
use smithay::desktop::{find_popup_root_surface, PopupKind};
use smithay::output::Output;
use smithay::reexports::calloop::Interest;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::{
    add_blocker, add_pre_commit_hook, get_parent, is_sync_subsurface, with_states,
    BufferAssignment, CompositorHandler, SurfaceAttributes,
};
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::XdgPopupSurfaceData;

use crate::state::{Fht, OutputState, State};

/// Ensures that the [`WlSurface`] has a render buffer
fn has_render_buffer(surface: &WlSurface) -> bool {
    // If there's no renderer surface data, just assume the surface didn't even get recognized by
    // the renderer
    with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false)
}

impl State {
    /// Process a commit request for a possible window toplevel.
    ///
    /// If this surface is actually associated with a window, this function will return the output
    /// associated where this window should be drawn.
    fn process_window_commit(surface: &WlSurface, state: &mut Fht) -> Option<Output> {
        if let Some(idx) = state
            .pending_windows
            .iter()
            .position(|w| w.inner.wl_surface().is_some_and(|s| &*s == surface))
        {
            let pending_window = state.pending_windows.get_mut(idx).unwrap();
            pending_window.inner.refresh();
            pending_window.inner.on_commit();

            // NOTE: We dont check whether the surface has a render buffer here, since most
            // toplevels dont attach one if their did not receive their initial configure.
            if !pending_window.initial_configure_sent {
                let pending_window = state.pending_windows.get_mut(idx).unwrap();
                // Send an empty configuration message so that the client informs us of new state.
                pending_window.inner.toplevel().unwrap().send_configure();
                pending_window.initial_configure_sent = true;

                return None;
            }

            let pending_window = state.pending_windows.remove(idx);
            state.prepare_pending_window(pending_window.inner);

            // FIXME: Why this doesn't commit by itself?
            let surface = surface.clone();
            state
                .loop_handle
                .insert_idle(move |state| state.commit(&surface));

            return None;
        }

        if let Some(idx) = state.unmapped_tiles.iter().position(|t| {
            t.inner
                .element()
                .wl_surface()
                .is_some_and(|s| &*s == surface)
        }) {
            let unmapped_tile = state.unmapped_tiles.get(idx).unwrap();
            unmapped_tile.inner.element().refresh();
            unmapped_tile.inner.element().on_commit();

            if !has_render_buffer(surface) {
                // FIXME: Why this doesn't commit by itself?
                let surface = surface.clone();
                state
                    .loop_handle
                    .insert_idle(move |state| state.commit(&surface));

                // We still cant map.
                return None;
            }

            // Otherwise now mapping is possible.
            let unmapped_tile = state.unmapped_tiles.remove(idx);
            let output = state.map_tile(unmapped_tile);

            return Some(output);
        }

        // Other check: its a mapped window.
        if let Some((window, output)) = state.find_window_and_output(surface) {
            window.on_commit();
            return Some(output.clone());
        }

        None
    }

    /// Process a popup surface commit request.
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

        if surface == &root_surface {
            // Committing a root surface, not a subsurface/popup.
            // Try to get the output where this surface is being drawn, otherwise quit.
            if let Some(output) = State::process_window_commit(&surface, &mut self.fht)
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
}

delegate_compositor!(State);
