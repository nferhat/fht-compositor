use fht_compositor_config::{BlurOverrides, ShadowOverrides};
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::desktop::{layer_map_for_output, LayerSurface, PopupManager};
use smithay::output::Output;
use smithay::utils::{Logical, Rectangle};

use crate::renderer::rounded_window::RoundedWindowElement;
use crate::renderer::shaders::ShaderElement;
use crate::renderer::{blur, FhtRenderer};
use crate::space::shadow::{self, Shadow};
use crate::state::Fht;

// FIXME: This removed blur but never added it back.

/// A mapped [`LayerSurface`].
#[derive(Debug)]
pub struct MappedLayer {
    /// The surface itself.
    pub layer: LayerSurface,
    /// The output of this layer surface.
    pub output: Output,
    /// The shadow surrounding this layer surface.
    pub shadow: Shadow,
    /// The resolved rules for this layer surface.
    pub rules: ResolvedLayerRules,
}

impl MappedLayer {
    pub fn new(
        layer: LayerSurface,
        output: Output,
        config: &fht_compositor_config::Config,
    ) -> Self {
        let rules = ResolvedLayerRules::resolve(&layer, &config.layer_rules, &output);
        let shadow = config.decorations.shadow.with_overrides(&rules.shadow);
        let shadow = Shadow::new(
            Rectangle::zero(), // NOTE: The actual geoemtry will be set on the first refresh call.
            shadow::Parameters {
                disable: shadow.disable,
                floating_only: false,
                color: shadow.color,
                blur_sigma: shadow.sigma,
                corner_radius: rules.corner_radius.unwrap_or(0.0),
            },
        );

        Self {
            layer,
            output,
            shadow,
            rules,
        }
    }

    pub fn refresh(
        &mut self,
        config: &fht_compositor_config::Config,
        layer_geo: Rectangle<i32, Logical>,
    ) {
        // Refresh the rules since this might be called after handling a layer-shell commit
        self.rules = ResolvedLayerRules::resolve(&self.layer, &config.layer_rules, &self.output);
        // And update the shadow since the layer geometry might change.
        self.shadow.set_geometry(layer_geo);
    }

    pub fn render<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        layer_geo: Rectangle<i32, Logical>,
        scale: i32,
        config: &fht_compositor_config::Config,
    ) -> impl Iterator<Item = LayerShellRenderElement<R>> {
        let wl_surface = self.layer.wl_surface();
        let render_geo = layer_geo.to_physical(scale);
        let alpha = self.rules.opacity.unwrap_or(1.0);

        // First render popups above everything else.
        let popups: Vec<_> = PopupManager::popups_for_surface(wl_surface)
            .flat_map(|(popup, popup_offset)| {
                let offset = (popup_offset - popup.geometry().loc).to_physical(scale);
                render_elements_from_surface_tree::<_, LayerShellRenderElement<_>>(
                    renderer,
                    popup.wl_surface(),
                    render_geo.loc + offset,
                    scale as f64,
                    alpha,
                    Kind::Unspecified,
                )
            })
            .collect();

        // Then render the actual surfaces
        let (mut normal, mut rounded) = (vec![], vec![]);
        let corner_radius = self.rules.corner_radius.unwrap_or(0.0);
        if corner_radius > 0.0 {
            rounded = render_elements_from_surface_tree(
                renderer,
                wl_surface,
                render_geo.loc,
                scale as f64,
                alpha,
                Kind::Unspecified,
            )
            .into_iter()
            .map(|surface| {
                if RoundedWindowElement::will_clip(&surface, scale as f64, layer_geo, corner_radius)
                {
                    RoundedWindowElement::new(surface, corner_radius, layer_geo, scale as f64)
                        .into()
                } else {
                    LayerShellRenderElement::Surface(surface)
                }
            })
            .collect();
        } else {
            normal = render_elements_from_surface_tree(
                renderer,
                wl_surface,
                render_geo.loc,
                scale as f64,
                alpha,
                Kind::Unspecified,
            );
        }

        let rv = None.into_iter().chain(popups).chain(normal).chain(rounded);

        // Now render decorations.
        let shadow = self
            .shadow
            .render(
                renderer, alpha, true, // doesn't matter with layer-shells?
            )
            .into_iter()
            .map(Into::into);
        let rv = rv.chain(shadow);

        // let blur = config.decorations.blur.with_overrides(&self.rules.blur);
        // let is_transparent = alpha < 1.0 || has_transparent_region(wl_surface, layer_geo.size);

        rv
    }
}

// Resolved layer rules that get computed from the configuration.
// They keep around actual values the user specified.
#[derive(Debug, Clone)]
pub struct ResolvedLayerRules {
    pub blur: BlurOverrides,
    pub corner_radius: Option<f32>,
    pub shadow: ShadowOverrides,
    pub opacity: Option<f32>,
}

impl Default for ResolvedLayerRules {
    fn default() -> Self {
        Self {
            blur: BlurOverrides {
                disable: Some(true),
                ..Default::default()
            },
            corner_radius: None,
            shadow: ShadowOverrides {
                disable: Some(true),
                ..Default::default()
            },
            opacity: None,
        }
    }
}

impl ResolvedLayerRules {
    pub fn resolve(
        layer: &LayerSurface,
        rules: &[fht_compositor_config::LayerRule],
        output: &Output,
    ) -> Self {
        crate::profile_function!();
        let mut resolved_rules = ResolvedLayerRules::default();

        for rule in rules
            .iter()
            .filter(|rule| rule_matches(rule, output, layer))
        {
            resolved_rules.shadow = resolved_rules.shadow.merge_with(&rule.shadow);
            resolved_rules.blur = resolved_rules.blur.merge_with(rule.blur);

            if let Some(opacity) = rule.opacity {
                resolved_rules.opacity = Some(opacity)
            }

            if let Some(corner_radius) = rule.corner_radius {
                resolved_rules.corner_radius = Some(corner_radius)
            }
        }

        resolved_rules
    }
}

fn rule_matches(
    rule: &fht_compositor_config::LayerRule,
    output: &Output,
    layer: &LayerSurface,
) -> bool {
    let namespace = layer.namespace();

    if rule.match_all {
        if rule
            .on_output
            .as_ref()
            .is_none_or(|name| name == &output.name())
        {
            return false;
        }

        rule.match_namespace
            .iter()
            .any(|regex| regex.is_match(namespace))
    } else {
        if rule
            .on_output
            .as_ref()
            .is_some_and(|name| name == &output.name())
        {
            return true;
        }

        if rule
            .match_namespace
            .iter()
            .any(|regex| regex.is_match(namespace))
        {
            return true;
        }

        false
    }
}

impl Fht {
    pub fn resolve_rules_for_all_layer_shells(&self) {
        for output in self.space.outputs() {
            let layer_map = layer_map_for_output(output);
            for layer in layer_map.layers() {
                ResolvedLayerRules::resolve(layer, &self.config.layer_rules, output);
            }
        }
    }
}

crate::fht_render_elements! {
    LayerShellRenderElement<R> => {
        Surface = WaylandSurfaceRenderElement<R>,
        RoundedSurface = RoundedWindowElement<R>,
        Blur = blur::BlurRegionRenderElement,
        Shadow = ShaderElement,
    }
}
