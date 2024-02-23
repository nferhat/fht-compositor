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
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .unwrap_or_else(|| self.fht.workspaces.keys().next().unwrap().clone());
        let layer_surface = LayerSurface::new(surface, namespace);
        let layer_surface = (layer_surface, output);
        self.fht.pending_layers.push(layer_surface);
    }

    fn layer_destroyed(&mut self, surface: wlr_layer::LayerSurface) {
        if let Some(idx) = self
            .fht
            .pending_layers
            .iter()
            .position(|(s, _)| s.layer_surface() == &surface)
        {
            let ret = self.fht.pending_layers.remove(idx);
            std::mem::drop(ret);
        } else if let Some((mut layer_map, layer)) = self.fht.outputs().find_map(|o| {
            let layer_map = layer_map_for_output(o);
            let layer = layer_map
                .layers()
                .find(|&layer| layer.layer_surface() == &surface)
                .cloned();
            layer.map(|l| (layer_map, l))
        }) {
            layer_map.unmap_layer(&layer)
        }
    }
}

delegate_layer_shell!(State);
