use std::sync::Mutex;

use smithay::backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state};
use smithay::delegate_compositor;
use smithay::desktop::{layer_map_for_output, LayerSurface, PopupKind, WindowSurfaceType};
use smithay::reexports::calloop::Interest;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::{
    add_blocker, add_pre_commit_hook, with_states, BufferAssignment, CompositorHandler,
    SurfaceAttributes,
};
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::LayerSurfaceAttributes;
use smithay::wayland::shell::xdg::{
    XdgPopupSurfaceRoleAttributes, XdgToplevelSurfaceRoleAttributes,
};

use crate::shell::FhtWindow;
use crate::state::{Fht, State};

/// Ensures that the [`WlSurface`] has a render buffer
fn has_render_buffer(surface: &WlSurface) -> bool {
    // If there's no renderer surface data, just assume the surface didn't even get recognized by
    // the renderer
    with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false)
}

impl State {
    /// Ensures that the initial configure event is sent for a toplevel, returning whether it was
    /// already sent or not.
    fn toplevel_ensure_initial_configure(window: &FhtWindow, state: &mut Fht) -> bool {
        let Some(toplevel) = window.0.toplevel() else {
            return false;
        };

        // Map the window
        state.map_window(window);

        let initial_configure_sent = with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });
        initial_configure_sent
    }
}

/// Ensures that the initial configure event is sent for a popup.
fn popup_ensure_initial_configure(popup: &PopupKind) {
    let PopupKind::Xdg(ref popup) = popup else {
        return;
    };

    let initial_configure_sent = with_states(popup.wl_surface(), |states| {
        states
            .data_map
            .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
            .unwrap()
            .lock()
            .unwrap()
            .initial_configure_sent
    });
    if !initial_configure_sent {
        // NOTE: A popup initial configure should never fail
        popup.send_configure().expect("Initial configure failed!");
    }
}

/// Ensures that the initial configure event is sent for a layer surface, returning whether it was
/// already sent or not.
fn layer_surface_ensure_initial_configure(surface: &LayerSurface) -> bool {
    let initial_configure_sent = with_states(surface.wl_surface(), |states| {
        states
            .data_map
            .get::<Mutex<LayerSurfaceAttributes>>()
            .unwrap()
            .lock()
            .unwrap()
            .initial_configure_sent
    });
    if !initial_configure_sent {
        surface.layer_surface().send_configure();
    }
    initial_configure_sent
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
                    .pending::<SurfaceAttributes>()
                    .buffer
                    .as_ref()
                    .and_then(|assignment| match assignment {
                        BufferAssignment::NewBuffer(buffer) => get_dmabuf(buffer).ok(),
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
        if let crate::backend::Backend::Udev(ref mut data) = &mut self.backend {
            data.early_import(surface);
        }

        if let Some(idx) = self
            .fht
            .pending_windows
            .iter()
            .position(|(w, _)| w.wl_surface().as_ref() == Some(surface))
        {
            let window = self
                .fht
                .pending_windows
                .get(idx)
                .map(|(w, _)| w)
                .unwrap()
                .clone();
            if State::toplevel_ensure_initial_configure(&window, &mut self.fht)
                && has_render_buffer(surface)
            {
                window.0.on_commit();
                self.fht.map_window(&window);
            } else {
                return;
            }
        }

        if let Some(idx) = self
            .fht
            .pending_layers
            .iter()
            .position(|(l, _)| l.wl_surface() == surface)
        {
            let (layer_surface, output) = self.fht.pending_layers.get(idx).unwrap();
            if layer_surface_ensure_initial_configure(layer_surface) {
                if let Err(err) = layer_map_for_output(&output).map_layer(layer_surface) {
                    warn!(?err, "Failed to map layer surface!");
                };
                let (_, output) = self.fht.pending_layers.remove(idx);
                self.fht.wset_for(&output).arrange();
            } else {
                return;
            }
        }

        if let Some(popup) = self.fht.popups.find_popup(surface) {
            popup_ensure_initial_configure(&popup);
        }

        if let Some(window) = self.fht.find_window(surface).filter(|w| w.is_wayland()) {
            window.0.on_commit();
        }

        self.fht.popups.commit(surface);

        let layer_output = self
            .fht
            .outputs()
            .find(|o| {
                let layer_map = layer_map_for_output(o);
                layer_map
                    .layer_for_surface(surface, WindowSurfaceType::ALL)
                    .is_some()
            })
            .cloned();
        if let Some(output) = layer_output {
            let has_arranged = layer_map_for_output(&output).arrange();
            if has_arranged {
                self.fht.wset_for(&output).arrange();
            }
        }

        if let Some(output) = self.fht.visible_output_for_surface(surface) {
            self.backend
                .schedule_render_output(output, &self.fht.loop_handle);
        }
    }
}

delegate_compositor!(State);
