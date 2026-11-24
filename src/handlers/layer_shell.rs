use smithay::delegate_layer_shell;
use smithay::desktop::{layer_map_for_output, LayerSurface, PopupKind, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Scale};
use smithay::wayland::compositor::{
    add_pre_commit_hook, remove_pre_commit_hook, with_states, BufferAssignment, SurfaceAttributes,
};
use smithay::wayland::shell::wlr_layer::{
    self, Layer, LayerSurfaceData, WlrLayerShellHandler, WlrLayerShellState,
};
use smithay::wayland::shell::xdg::PopupSurface;

use super::compositor::has_render_buffer;
use crate::layer::MappedLayer;
use crate::renderer::blur::EffectsFramebuffers;
use crate::state::State;

fn add_layer_pre_commit_hook(layer_surface: &LayerSurface) -> smithay::wayland::compositor::HookId {
    let wl_surface = layer_surface.wl_surface();
    add_pre_commit_hook::<State, _>(wl_surface, |state, _dh, surface| {
        let got_unmapped = with_states(surface, |states| {
            let mut guard = states.cached_state.get::<SurfaceAttributes>();
            let attrs = guard.pending();
            matches!(attrs.buffer, Some(BufferAssignment::Removed))
        });

        let Some(mapped) = state
            .fht
            .mapped_layer_surfaces
            .values_mut()
            .find(|m| m.layer.wl_surface() == surface)
        else {
            return;
        };

        if got_unmapped {
            let scale = mapped.output.current_scale().integer_scale();
            let config = state.fht.config.clone();
            state.backend.with_renderer(|renderer| {
                mapped.prepare_close_animation_if_needed(renderer, scale, &config);
            });
        } else {
            mapped.clear_close_animation_snapshot();
        }
    })
}

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.fht.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: wlr_layer::LayerSurface,
        output: Option<wl_output::WlOutput>,
        wlr_layer: wlr_layer::Layer,
        namespace: String,
    ) {
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .unwrap_or_else(|| self.fht.space.active_output().clone());
        let layer_surface = LayerSurface::new(surface, namespace);

        if matches!(wlr_layer, Layer::Background | Layer::Bottom) {
            // the optimized blur buffer has been dirtied, re-render on next State::dispatch
            EffectsFramebuffers::get(&output).optimized_blur_dirty = true;
        }

        let mut map = layer_map_for_output(&output);
        if let Err(err) = map.map_layer(&layer_surface.clone()) {
            error!(?err, "Failed to map layer-shell");
        }

        let hook_id = add_layer_pre_commit_hook(&layer_surface);
        let mut mapped = MappedLayer::new(layer_surface.clone(), output.clone(), &self.fht.config);
        mapped.set_pre_commit_hook_id(hook_id);
        self.fht.mapped_layer_surfaces.insert(layer_surface, mapped);
    }

    fn layer_destroyed(&mut self, surface: wlr_layer::LayerSurface) {
        let mut layer_output = None;
        if let Some((mut layer_map, layer, output)) = self.fht.space.outputs().find_map(|o| {
            let layer_map = layer_map_for_output(o);
            let layer = layer_map
                .layers()
                .find(|&layer| layer.layer_surface() == &surface)
                .cloned();
            layer.map(|l| (layer_map, l, o.clone()))
        }) {
            let maybe_geo = layer_map.layer_geometry(&layer);
            // Otherwise, it was already mapped, unmap it then close
            layer_map.unmap_layer(&layer);
            layer.layer_surface().send_close();
            if let Some(mut mapped) = self.fht.mapped_layer_surfaces.remove(&layer) {
                if let Some(geo) = maybe_geo {
                    mapped.geometry = geo;
                }
                if let Some(hook_id) = mapped.take_pre_commit_hook_id() {
                    remove_pre_commit_hook(layer.wl_surface(), &hook_id);
                }
                let scale = output.current_scale().integer_scale();
                let config = self.fht.config.clone();
                let mut closing_layer = None;
                self.backend.with_renderer(|renderer| {
                    mapped.prepare_close_animation_if_needed(renderer, scale, &config);
                    closing_layer = mapped.into_closing_layer(
                        renderer,
                        Scale::from(scale as f64),
                        &config,
                    );
                });
                if let Some(closing) = closing_layer {
                    self.fht.closing_layer_surfaces.push(closing);
                }
            }

            if matches!(layer.layer(), Layer::Background | Layer::Bottom) {
                // the optimized blur buffer has been dirtied, re-render on next State::dispatch
                EffectsFramebuffers::get(&output).optimized_blur_dirty = true;
            }

            layer_output = Some(output);
        }

        if let Some(output) = layer_output {
            self.fht.output_resized(&output);
        }
    }

    fn new_popup(&mut self, parent: wlr_layer::LayerSurface, popup: PopupSurface) {
        let desktop_layer = self.fht.space.outputs().find_map(|output| {
            let layer_map = layer_map_for_output(output);
            let layer = layer_map
                .layers()
                .find(|layer| layer.layer_surface() == &parent)?;
            Some((layer.clone(), output.clone()))
        });

        if let Some((parent_layer, output)) = desktop_layer {
            self.fht
                .unconstrain_layer_popup(&popup, &parent_layer, &output);
            if let Err(err) = self.fht.popups.track_popup(PopupKind::from(popup)) {
                tracing::warn!(?err, "Failed to track layer shell popup!");
            }
        }
    }
}

impl State {
    pub fn process_layer_shell_commit(&mut self, surface: &WlSurface) -> Option<Output> {
        let mut layer_output = None;
        let mut unmapped_layer_key = None;
        if let Some(output) = self
            .fht
            .space
            .outputs()
            .find(|o| {
                let map = layer_map_for_output(o);
                map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                    .is_some()
            })
            .cloned()
        {
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

            let changed = map.arrange();
            layer_output = Some((output.clone(), changed));

            let layer = map
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .unwrap()
                .clone();
            let layer_geo = map.layer_geometry(&layer).unwrap();
            drop(map);

            // send the initial configure if relevant
            if !initial_configure_sent {
                if matches!(layer.layer(), Layer::Background | Layer::Bottom) {
                    // the optimized blur buffer has been dirtied, re-render on next State::dispatch
                    EffectsFramebuffers::get(&output).optimized_blur_dirty = true;
                }

                layer.layer_surface().send_configure();
            }

            let is_mapped = surface.alive() && has_render_buffer(surface);
            if let Some(mapped_layer) = self.fht.mapped_layer_surfaces.get_mut(&layer) {
                mapped_layer.refresh(&self.fht.config, layer_geo);
                if !mapped_layer.is_mapped && is_mapped {
                    mapped_layer.is_mapped = true;
                    mapped_layer.start_opening_animation(&self.fht.config);
                    let ns = layer.namespace();
                    self.fht
                        .closing_layer_surfaces
                        .retain(|c| !(c.output == output && c.namespace == ns));
                } else if mapped_layer.is_mapped && !is_mapped {
                    mapped_layer.is_mapped = false;
                    unmapped_layer_key = Some((layer, output.clone()));
                }
            }
        }

        if let Some((layer, output)) = unmapped_layer_key {
            let scale = Scale::from(output.current_scale().integer_scale() as f64);
            let config = self.fht.config.clone();
            if let Some(mapped_layer) = self.fht.mapped_layer_surfaces.get_mut(&layer) {
                let mut closing_layer = None;
                self.backend.with_renderer(|renderer| {
                    closing_layer = mapped_layer.take_closing_layer(renderer, scale, &config);
                });
                if let Some(closing) = closing_layer {
                    self.fht.closing_layer_surfaces.push(closing);
                }
            }
        }

        if let Some((output, changed)) = layer_output.as_ref() {
            if *changed {
                self.fht.space.output_resized(output, true);
            }
        }

        layer_output.map(|(o, _)| o)
    }
}

delegate_layer_shell!(State);
