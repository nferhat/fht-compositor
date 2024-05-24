use smithay::delegate_layer_shell;
use smithay::desktop::{layer_map_for_output, LayerSurface, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::wlr_layer::{
    self, LayerSurfaceData, WlrLayerShellHandler, WlrLayerShellState,
};

use crate::state::{Fht, State};

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.fht.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: wlr_layer::LayerSurface,
        output: Option<wl_output::WlOutput>,
        _layer: wlr_layer::Layer,
        namespace: String,
    ) {
        // We don't map layer surfaces immediatly, rather, they get pushed to `pending_layers`
        // before mapping. The compositors waits for the initial configure of the layer surface
        // before mapping so we are sure it have dimensions and a render buffer
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .unwrap_or_else(|| self.fht.workspaces.keys().next().unwrap().clone());
        let layer_surface = LayerSurface::new(surface, namespace);
        let mut map = layer_map_for_output(&output);
        map.map_layer(&layer_surface)
            .expect("Failed to map layer shell!");
    }

    fn layer_destroyed(&mut self, surface: wlr_layer::LayerSurface) {
        let mut layer_output = None;
        if let Some((mut layer_map, layer, output)) = self.fht.outputs().find_map(|o| {
            let layer_map = layer_map_for_output(o);
            let layer = layer_map
                .layers()
                .find(|&layer| layer.layer_surface() == &surface)
                .cloned();
            layer.map(|l| (layer_map, l, o.clone()))
        }) {
            // Otherwise, it was already mapped, unmap it then close
            layer_map.unmap_layer(&layer);
            layer.layer_surface().send_close();
            layer_output = Some(output);
        }

        if let Some(output) = layer_output {
            self.fht.output_resized(&output);
        }
    }
}

impl State {
    /// Process a potential commit request for a layer shell
    ///
    /// Returns the output holding the layer shell associated with this surface.
    pub fn process_layer_shell_commit(surface: &WlSurface, state: &mut Fht) -> Option<Output> {
        let mut layer_output = None;
        if let Some(output) = state.outputs().find(|o| {
            let map = layer_map_for_output(o);
            map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .is_some()
        }) {
            layer_output = Some(output.clone());
            let initial_configure_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<LayerSurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .initial_configure_sent
            });

            let mut map = layer_map_for_output(&output);

            // arrange the layers before sending the initial configure
            // to respect any size the client may have sent
            map.arrange();
            // send the initial configure if relevant
            if !initial_configure_sent {
                let layer = map
                    .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                    .unwrap();

                layer.layer_surface().send_configure();
            }
        }
        if let Some(output) = layer_output.as_ref() {
            // fighting rust's borrow checker episode 32918731287
            state.output_resized(output);
        }

        layer_output
    }
}

delegate_layer_shell!(State);
