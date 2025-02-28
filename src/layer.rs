use std::cell::{Ref, RefCell};
use std::rc::Rc;

use fht_compositor_config::{BlurOverrides, ShadowOverrides};
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::AsRenderElements;
use smithay::desktop::{layer_map_for_output, LayerSurface};
use smithay::output::Output;
use smithay::wayland::shell::wlr_layer;

use crate::renderer::blur::element::BlurElement;
use crate::renderer::pixel_shader_element::FhtPixelShaderElement;
use crate::renderer::{has_transparent_region, FhtRenderer};
use crate::state::Fht;

// Resolved layer rules that get computed from the configuration.
// They keep around actual values the user specified.
#[derive(Debug, Clone, Default)]
pub struct ResolvedLayerRules {
    pub blur: BlurOverrides,
    pub shadow: ShadowOverrides,
    pub opacity: Option<f32>,
}

impl ResolvedLayerRules {
    pub fn resolve(
        layer: &LayerSurface,
        rules: &[fht_compositor_config::LayerRule],
        output: &Output,
    ) {
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
        }

        let guard = layer
            .user_data()
            .get_or_insert(|| Rc::new(RefCell::new(LayerRuleState::default())))
            .clone();
        let mut guard = RefCell::borrow_mut(&guard);
        guard.needs_resolving = false;
        guard.resolved = resolved_rules;
    }

    pub fn get(layer: &LayerSurface) -> Ref<'_, ResolvedLayerRules> {
        let guard = layer
            .user_data()
            .get_or_insert(|| Rc::new(RefCell::new(LayerRuleState::default())));
        let guard = RefCell::borrow(guard);
        Ref::map(guard, |guard| &guard.resolved)
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
            .is_some_and(|name| name != &output.name())
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

#[derive(Debug, Default)]
struct LayerRuleState {
    pub needs_resolving: bool,
    pub resolved: ResolvedLayerRules,
}

pub fn layer_elements<R: FhtRenderer, C>(
    renderer: &mut R,
    output: &Output,
    layer: wlr_layer::Layer,
    config: &fht_compositor_config::Config,
) -> impl Iterator<Item = C>
where
    C: From<LayerShellRenderElement<R>>,
{
    crate::profile_function!();
    let output_scale = output.current_scale().integer_scale();
    let layer_map = layer_map_for_output(output);
    let mut elements: Vec<LayerShellRenderElement<R>> = vec![];

    for layer in layer_map.layers_on(layer).rev() {
        let rules = ResolvedLayerRules::get(layer);
        let alpha = rules.opacity.unwrap_or(1.0);
        let wl_surface = layer.wl_surface();
        let layer_geo = layer_map.layer_geometry(layer).unwrap();

        let blur = config.decorations.blur.with_overrides(&rules.blur);
        let shadow = config.decorations.shadow.with_overrides(&rules.shadow);

        let location = layer_geo.loc.to_physical_precise_round(output_scale);
        elements.extend(layer.render_elements(
            renderer,
            location,
            (output_scale as f64).into(),
            alpha,
        ));

        let is_transparent = rules.opacity.map_or_else(
            || has_transparent_region(wl_surface, layer_geo.size),
            |o| o < 1.0,
        );
        if !blur.disabled() && is_transparent {
            let blur_element = BlurElement::new(
                renderer,
                output,
                layer_geo,
                location,
                0.0,
                false, // FIXME: Configurable
                output_scale,
                1.0,
                blur,
            );

            elements.push(blur_element.into());
        }

        if !shadow.disable && shadow.color[3] > 0.0 {
            let element = crate::space::decorations::draw_shadow(
                renderer,
                alpha,
                output_scale,
                layer_geo,
                shadow.sigma,
                0.0,
                shadow.color,
            );
            elements.push(element.into());
        }
    }

    elements.into_iter().map(Into::into)
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
        Blur = BlurElement,
        Shadow = FhtPixelShaderElement,
    }
}
