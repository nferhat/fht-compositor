use smithay::delegate_layer_shell;
use smithay::desktop::{layer_map_for_output, LayerSurface};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output;
use smithay::wayland::shell::wlr_layer::{self, WlrLayerShellHandler, WlrLayerShellState};

use crate::state::State;

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
        let wl_surface = surface.wl_surface().clone();
        let layer_surface = LayerSurface::new(surface, namespace);
        self.fht
            .pending_layers
            .insert(wl_surface, (layer_surface, output));
    }

    fn layer_destroyed(&mut self, surface: wlr_layer::LayerSurface) {
        let mut layer_output = None;
        if let Some((layer_surface, _)) = self.fht.pending_layers.remove(surface.wl_surface()) {
            // This was a pending layer, it was not mapped, just close it.
            layer_surface.layer_surface().send_close();
        } else if let Some((mut layer_map, layer, output)) = self.fht.outputs().find_map(|o| {
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

delegate_layer_shell!(State);
