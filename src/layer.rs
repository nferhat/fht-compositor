use std::time::Duration;

use fht_animation::Animation;
use fht_compositor_config::{BlurOverrides, LayerAnimationStyle, ShadowOverrides};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::utils::{
    Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::{Element as _, Id, Kind};
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::Renderer as _;
use smithay::desktop::{LayerSurface, PopupManager};
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::compositor::HookId;
use smithay::wayland::shell::wlr_layer::{Anchor, Layer};

use crate::output::OutputExt;
use crate::renderer::blur::element::BlurElement;
use crate::renderer::rounded_window::RoundedWindowElement;
use crate::renderer::shaders::ShaderElement;
use crate::renderer::surface::push_elements_from_surface_tree;
use crate::renderer::texture_element::FhtTextureElement;
use crate::renderer::{has_transparent_region, render_to_texture, FhtRenderer};
use crate::space::shadow::{self, Shadow};
use crate::state::Fht;
use crate::utils::RectCenterExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlideDirection {
    Top,
    Bottom,
    Left,
    Right,
}

fn resolve_slide_direction(
    anchor: Anchor,
    layer_geo: Rectangle<i32, Logical>,
    output_size: Size<i32, Logical>,
) -> SlideDirection {
    let top = anchor.contains(Anchor::TOP);
    let bottom = anchor.contains(Anchor::BOTTOM);
    let left = anchor.contains(Anchor::LEFT);
    let right = anchor.contains(Anchor::RIGHT);

    if top && !bottom {
        SlideDirection::Top
    } else if bottom && !top {
        SlideDirection::Bottom
    } else if left && !right {
        SlideDirection::Left
    } else if right && !left {
        SlideDirection::Right
    } else {
        let center_y = layer_geo.loc.y + layer_geo.size.h / 2;
        if center_y > output_size.h / 2 {
            SlideDirection::Bottom
        } else {
            SlideDirection::Top
        }
    }
}

fn compute_layer_animation_transform(
    style: LayerAnimationStyle,
    progress: f64,
    layer_geo: Rectangle<i32, Logical>,
    output_size: Size<i32, Logical>,
    anchor: Anchor,
) -> (Point<i32, Logical>, f64, f32) {
    let alpha_mult = (progress as f32).clamp(0.0, 1.0);
    match style {
        LayerAnimationStyle::None => (Point::default(), 1.0, 1.0),
        LayerAnimationStyle::Fade => (Point::default(), 1.0, alpha_mult),
        LayerAnimationStyle::Popin { min_scale } => {
            let scale = (progress * (1.0 - min_scale) + min_scale).max(0.01);
            (Point::default(), scale, alpha_mult)
        }
        LayerAnimationStyle::Slide
        | LayerAnimationStyle::SlideTop
        | LayerAnimationStyle::SlideBottom
        | LayerAnimationStyle::SlideLeft
        | LayerAnimationStyle::SlideRight => {
            let dir = match style {
                LayerAnimationStyle::SlideTop => SlideDirection::Top,
                LayerAnimationStyle::SlideBottom => SlideDirection::Bottom,
                LayerAnimationStyle::SlideLeft => SlideDirection::Left,
                LayerAnimationStyle::SlideRight => SlideDirection::Right,
                _ => resolve_slide_direction(anchor, layer_geo, output_size),
            };
            let full_offset = match dir {
                SlideDirection::Top => Point::from((0, -(layer_geo.loc.y + layer_geo.size.h))),
                SlideDirection::Bottom => Point::from((0, output_size.h - layer_geo.loc.y)),
                SlideDirection::Left => Point::from((-(layer_geo.loc.x + layer_geo.size.w), 0)),
                SlideDirection::Right => Point::from((output_size.w - layer_geo.loc.x, 0)),
            };
            let offset = full_offset.to_f64().upscale(1.0 - progress).to_i32_round();
            (offset, 1.0, alpha_mult)
        }
    }
}

/// A closing [`LayerSurface`] animation snapshot.
#[derive(Debug)]
pub struct ClosingLayer {
    pub output: Output,
    pub wlr_layer: Layer,
    pub namespace: String,
    texture: Option<(TextureBuffer<GlesTexture>, Point<i32, Logical>)>,
    geometry: Rectangle<i32, Logical>,
    output_size: Size<i32, Logical>,
    anchor: Anchor,
    base_alpha: f32,
    style: LayerAnimationStyle,
    progress: Animation<f64>,
}

impl ClosingLayer {
    pub fn new(
        renderer: &mut GlowRenderer,
        render_elements: Vec<LayerShellRenderElement<GlowRenderer>>,
        geometry: Rectangle<i32, Logical>,
        output: Output,
        wlr_layer: Layer,
        namespace: String,
        anchor: Anchor,
        base_alpha: f32,
        style: LayerAnimationStyle,
        initial_progress: f64,
        scale: Scale<f64>,
        config: &fht_compositor_config::Config,
    ) -> Option<Self> {
        if style == LayerAnimationStyle::None
            || config.animations.disable
            || config.animations.layer_open_close.disable
        {
            return None;
        }

        let geo = render_elements
            .iter()
            .fold(Rectangle::default(), |acc, e| acc.merge(e.geometry(scale)));
        if geo.size.w <= 0 || geo.size.h <= 0 {
            return None;
        }

        let render_elements = render_elements.into_iter().rev().map(|e| {
            RelocateRenderElement::from_element(e, (-geo.loc.x, -geo.loc.y), Relocate::Relative)
        });

        let texture = match render_to_texture(
            renderer,
            geo.size,
            scale,
            Transform::Normal,
            Fourcc::Abgr8888,
            render_elements,
        ) {
            Ok((texture, _)) => {
                let texture = TextureBuffer::from_texture(
                    renderer,
                    texture,
                    scale.x.max(scale.y) as i32,
                    Transform::Normal,
                    None,
                );
                Some((texture, geo.loc.to_f64().to_logical(scale).to_i32_round()))
            }
            Err(err) => {
                warn!(?err, "Failed to render texture for ClosingLayer");
                return None;
            }
        };

        let anim_cfg = &config.animations.layer_open_close;
        let output_size = output.geometry().size;
        let start = initial_progress.clamp(0.01, 1.0);
        Some(Self {
            output,
            wlr_layer,
            namespace,
            texture,
            geometry,
            output_size,
            anchor,
            base_alpha,
            style,
            progress: Animation::new(start, 0.0, anim_cfg.duration).with_curve(anim_cfg.curve),
        })
    }

    pub fn advance_animations(&mut self, target_presentation_time: Duration) {
        self.progress.tick(target_presentation_time);
    }

    pub fn is_finished(&self) -> bool {
        self.progress.is_finished()
    }

    pub fn render<R: FhtRenderer>(&self, scale: i32) -> Option<LayerShellRenderElement<R>> {
        let (texture, offset) = self.texture.as_ref()?;
        let progress = *self.progress.value();
        let (slide_offset, popin_scale, alpha_mult) = compute_layer_animation_transform(
            self.style,
            progress,
            self.geometry,
            self.output_size,
            self.anchor,
        );

        let texture: FhtTextureElement = TextureRenderElement::from_texture_buffer(
            Point::from((0., 0.)),
            texture,
            Some((alpha_mult * self.base_alpha).clamp(0.0, 1.0)),
            None,
            None,
            Kind::Unspecified,
        )
        .into();

        let center = self.geometry.size.to_point().downscale(2);
        let origin = (center - *offset).to_physical_precise_round(scale);
        let rescale = RescaleRenderElement::from_element(texture, origin, popin_scale);

        let location = (self.geometry.loc + slide_offset + *offset).to_physical_precise_round(scale);
        let relocate = RelocateRenderElement::from_element(rescale, location, Relocate::Relative);
        Some(LayerShellRenderElement::Closing(relocate))
    }
}

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
    /// Cached geometry of this layer surface.
    pub geometry: Rectangle<i32, Logical>,
    /// Whether this layer surface currently has a committed render buffer.
    pub is_mapped: bool,
    /// Current opening animation progress (0.0 -> 1.0).
    opening_animation: Option<Animation<f64>>,
    /// Prepared render elements for a [`ClosingLayer`], along with the opening progress at snapshot time.
    close_animation_snapshot: Option<(Vec<LayerShellRenderElement<GlowRenderer>>, f64)>,
    /// Pre-commit hook ID on the layer's root WlSurface.
    pre_commit_hook_id: Option<HookId>,
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
                corner_power: rules.corner_power.unwrap_or(2.0),
            },
        );

        Self {
            layer,
            output,
            shadow,
            rules,
            geometry: Rectangle::zero(),
            is_mapped: false,
            opening_animation: None,
            close_animation_snapshot: None,
            pre_commit_hook_id: None,
        }
    }

    pub fn set_pre_commit_hook_id(&mut self, hook_id: HookId) {
        self.pre_commit_hook_id = Some(hook_id);
    }

    pub fn take_pre_commit_hook_id(&mut self) -> Option<HookId> {
        self.pre_commit_hook_id.take()
    }

    pub fn effective_animation_style(
        &self,
        config: &fht_compositor_config::Config,
    ) -> LayerAnimationStyle {
        if config.animations.disable || config.animations.layer_open_close.disable {
            return LayerAnimationStyle::None;
        }
        self.rules
            .animation
            .unwrap_or(config.animations.layer_open_close.style)
    }

    pub fn start_opening_animation(&mut self, config: &fht_compositor_config::Config) {
        if self.effective_animation_style(config) == LayerAnimationStyle::None {
            self.opening_animation = None;
            return;
        }
        let anim_cfg = &config.animations.layer_open_close;
        self.opening_animation =
            Some(Animation::new(0.0, 1.0, anim_cfg.duration).with_curve(anim_cfg.curve));
    }

    pub fn advance_animations(&mut self, target_presentation_time: Duration) -> bool {
        let _ = self.opening_animation.take_if(|a| a.is_finished());
        if let Some(animation) = &mut self.opening_animation {
            animation.tick(target_presentation_time);
            return true;
        }
        false
    }

    pub fn refresh(
        &mut self,
        config: &fht_compositor_config::Config,
        layer_geo: Rectangle<i32, Logical>,
    ) {
        // Refresh the rules since this might be called after handling a layer-shell commit
        self.rules = ResolvedLayerRules::resolve(&self.layer, &config.layer_rules, &self.output);
        self.geometry = layer_geo;
        // And update the shadow since the layer geometry might change.
        let shadow = config.decorations.shadow.with_overrides(&self.rules.shadow);
        self.shadow.update_parameters(shadow::Parameters {
            disable: shadow.disable,
            floating_only: false,
            color: shadow.color,
            blur_sigma: shadow.sigma,
            corner_radius: self.rules.corner_radius.unwrap_or(0.0),
            corner_power: self.rules.corner_power.unwrap_or(2.0),
        });
        self.shadow.set_geometry(layer_geo);
    }

    pub fn prepare_close_animation_if_needed(
        &mut self,
        renderer: &mut GlowRenderer,
        scale: i32,
        config: &fht_compositor_config::Config,
    ) {
        if self.close_animation_snapshot.is_some()
            || !self.is_mapped
            || self.geometry.size.w <= 0
            || self.geometry.size.h <= 0
        {
            return;
        }

        if self.effective_animation_style(config) == LayerAnimationStyle::None {
            return;
        }

        let mut elements = vec![];
        let local_geo = Rectangle::from_size(self.geometry.size);
        self.render_inner(renderer, local_geo, scale, 1.0, &mut |e| elements.push(e));
        if !elements.is_empty() {
            let current_progress = self
                .opening_animation
                .as_ref()
                .map(|a| *a.value())
                .unwrap_or(1.0);
            self.close_animation_snapshot = Some((elements, current_progress));
        }
    }

    pub fn clear_close_animation_snapshot(&mut self) {
        let _ = self.close_animation_snapshot.take();
    }

    pub fn take_closing_layer(
        &mut self,
        renderer: &mut GlowRenderer,
        scale: Scale<f64>,
        config: &fht_compositor_config::Config,
    ) -> Option<ClosingLayer> {
        let (render_elements, initial_progress) = self.close_animation_snapshot.take()?;
        let style = self.effective_animation_style(config);
        let anchor = self.layer.cached_state().anchor;
        let wlr_layer = self.layer.layer();
        let namespace = self.layer.namespace().to_string();
        let base_alpha = self.rules.opacity.unwrap_or(1.0);

        ClosingLayer::new(
            renderer,
            render_elements,
            self.geometry,
            self.output.clone(),
            wlr_layer,
            namespace,
            anchor,
            base_alpha,
            style,
            initial_progress,
            scale,
            config,
        )
    }

    pub fn into_closing_layer(
        mut self,
        renderer: &mut GlowRenderer,
        scale: Scale<f64>,
        config: &fht_compositor_config::Config,
    ) -> Option<ClosingLayer> {
        self.take_closing_layer(renderer, scale, config)
    }

    fn render_inner<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        layer_geo: Rectangle<i32, Logical>,
        scale: i32,
        alpha: f32,
        push: &mut dyn FnMut(LayerShellRenderElement<R>),
    ) {
        let wl_surface = self.layer.wl_surface();
        let render_geo = layer_geo.to_physical(scale);

        // First render popups above everything else.
        for (popup, popup_offset) in PopupManager::popups_for_surface(wl_surface) {
            let offset = (popup_offset - popup.geometry().loc).to_physical(scale);
            push_elements_from_surface_tree::<_>(
                renderer,
                popup.wl_surface(),
                render_geo.loc + offset,
                scale as f64,
                alpha,
                Kind::Unspecified,
                &mut |e| push(e.into()),
            )
        }

        // Then render the actual surfaces
        let radius = self.rules.corner_radius.unwrap_or(0.0);
        let power = self.rules.corner_power.unwrap_or(2.0);
        if radius > 0.0 {
            push_elements_from_surface_tree(
                renderer,
                wl_surface,
                render_geo.loc,
                scale as f64,
                alpha,
                Kind::Unspecified,
                &mut |surface| {
                    if RoundedWindowElement::will_clip(&surface, scale as f64, layer_geo, radius) {
                        let rounded = RoundedWindowElement::new(
                            surface,
                            radius,
                            power,
                            layer_geo,
                            scale as f64,
                        );
                        push(LayerShellRenderElement::RoundedSurface(rounded))
                    } else {
                        push(LayerShellRenderElement::Surface(surface))
                    }
                },
            );
        } else {
            push_elements_from_surface_tree(
                renderer,
                wl_surface,
                render_geo.loc,
                scale as f64,
                alpha,
                Kind::ScanoutCandidate,
                &mut |e| push(e.into()),
            );
        }

        if let Some(shadow) = self.shadow.render(renderer, alpha, true) {
            let mut expanded = layer_geo;
            let shadow::Parameters { blur_sigma, .. } = *self.shadow.parameters();
            expanded.loc -= Point::from((blur_sigma as i32, blur_sigma as i32));
            expanded.size += Size::from((blur_sigma as i32, blur_sigma as i32)).upscale(2);
            let element = shadow.with_location(expanded.loc).with_size(expanded.size);
            push(LayerShellRenderElement::Shadow(element))
        }
    }

    pub fn render<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        layer_geo: Rectangle<i32, Logical>,
        scale: i32,
        config: &fht_compositor_config::Config,
        push: &mut dyn FnMut(LayerShellRenderElement<R>),
    ) {
        let wl_surface = self.layer.wl_surface();
        let base_alpha = self.rules.opacity.unwrap_or(1.0);
        let radius = self.rules.corner_radius.unwrap_or(0.0);
        let power = self.rules.corner_power.unwrap_or(2.0);

        let (slide_offset, popin_scale, alpha_mult) =
            if let Some(animation) = self.opening_animation.as_ref() {
                let progress = *animation.value();
                let style = self.effective_animation_style(config);
                let output_size = self.output.geometry().size;
                let anchor = self.layer.cached_state().anchor;
                compute_layer_animation_transform(style, progress, layer_geo, output_size, anchor)
            } else {
                (Point::default(), 1.0, 1.0)
            };

        let alpha = (base_alpha * alpha_mult).clamp(0.0, 1.0);

        if (popin_scale - 1.0).abs() > 1e-4 {
            let fractional_scale = Scale::from(scale as f64);
            let glow_renderer = renderer.glow_renderer_mut();
            let mut elements = vec![];
            let local_geo = Rectangle::from_size(layer_geo.size);
            self.render_inner(glow_renderer, local_geo, scale, 1.0, &mut |e| {
                elements.push(e)
            });
            let rec = elements
                .iter()
                .fold(Rectangle::default(), |acc, e| acc.merge(e.geometry(fractional_scale)));

            if rec.size.w > 0 && rec.size.h > 0 {
                let relocated = elements.into_iter().rev().map(|e| {
                    RelocateRenderElement::from_element(
                        e,
                        (-rec.loc.x, -rec.loc.y),
                        Relocate::Relative,
                    )
                });
                if let Ok((tex, _sync_point)) = render_to_texture(
                    glow_renderer,
                    rec.size,
                    fractional_scale,
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    relocated,
                ) {
                    let glow_renderer = renderer.glow_renderer_mut();
                    let offset = rec
                        .loc
                        .to_f64()
                        .to_logical(fractional_scale)
                        .to_i32_round();
                    let element_id = Id::new();
                    let texture: FhtTextureElement = TextureRenderElement::from_static_texture(
                        element_id,
                        glow_renderer.context_id(),
                        (layer_geo.loc + offset).to_physical(scale).to_f64(),
                        tex,
                        scale,
                        Transform::Normal,
                        Some(alpha),
                        None,
                        None,
                        None,
                        Kind::Unspecified,
                    )
                    .into();

                    let origin = layer_geo.to_physical(scale).center();
                    let rescale = RescaleRenderElement::from_element(texture, origin, popin_scale);
                    push(LayerShellRenderElement::Opening(rescale));
                }
            }
        } else {
            let animated_geo = Rectangle::new(layer_geo.loc + slide_offset, layer_geo.size);
            self.render_inner(renderer, animated_geo, scale, alpha, push);
        }

        let blur = config.decorations.blur.with_overrides(&self.rules.blur);
        let is_transparent = alpha < 1.0 || has_transparent_region(wl_surface, layer_geo.size);
        if !blur.disabled() && is_transparent {
            let element = BlurElement::new(
                renderer,
                &self.output,
                layer_geo,
                layer_geo.loc.to_physical(scale),
                radius,
                power,
                false, // FIXME: Configurable
                scale,
                alpha,
                blur,
            );
            push(element.into())
        }
    }
}

// Resolved layer rules that get computed from the configuration.
// They keep around actual values the user specified.
#[derive(Debug, Clone)]
pub struct ResolvedLayerRules {
    pub blur: BlurOverrides,
    pub corner_radius: Option<f32>,
    pub corner_power: Option<f32>,
    pub shadow: ShadowOverrides,
    pub opacity: Option<f32>,
    pub animation: Option<LayerAnimationStyle>,
}

impl Default for ResolvedLayerRules {
    fn default() -> Self {
        Self {
            blur: BlurOverrides {
                disable: Some(true),
                ..Default::default()
            },
            corner_radius: None,
            corner_power: None,
            shadow: ShadowOverrides {
                disable: Some(true),
                ..Default::default()
            },
            opacity: None,
            animation: None,
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

            if let Some(animation) = rule.animation {
                resolved_rules.animation = Some(animation);
            }

            if let Some(no_anim) = rule.no_anim {
                if no_anim {
                    resolved_rules.animation = Some(LayerAnimationStyle::None);
                }
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
    pub fn resolve_rules_for_all_layer_shells(&mut self) {
        for mapped in self.mapped_layer_surfaces.values_mut() {
            let geo = mapped.geometry;
            mapped.refresh(&self.config, geo);
        }
    }
}

crate::fht_render_elements! {
    LayerShellRenderElement<R> => {
        Surface = WaylandSurfaceRenderElement<R>,
        RoundedSurface = RoundedWindowElement<R>,
        Blur = BlurElement,
        Shadow = ShaderElement,
        Opening = RescaleRenderElement<FhtTextureElement>,
        Closing = RelocateRenderElement<RescaleRenderElement<FhtTextureElement>>,
    }
}
