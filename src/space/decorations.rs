//! Decorations rendering.
//!
//! This is achieved using a GlesPixelShader, nothing special otherwise.

use std::cell::OnceCell;
use std::time::Duration;

use fht_animation::Animation;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::Uniform;
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::AnimationConfig;
use crate::renderer::pixel_shader_element::FhtPixelShaderElement;
use crate::renderer::shaders::Shaders;
use crate::renderer::{AsGlowRenderer, FhtRenderer};

/// A border around a tile.
///
/// The border uses a [`FhtPixelShaderElement`] under the hood to do the drawing. The border grows
/// *inwards*, not outwards. You can use [`Border::expand_rect`] and [`Border::shrink_rect`] to
/// adapt [`Rectangle`]s based on different thicknesses.
#[derive(Debug)]
pub struct Border {
    /// The underlying [`PixelShaderElement`] that draws the border.
    // Use a OnceCell to not require a `impl FhtRenderer` for creating Tiles
    element: OnceCell<FhtPixelShaderElement>,
    /// The geometry of the border.
    geometry: Rectangle<i32, Logical>,
    /// The outer corner radius of the border.
    corner_radius: Animation<f32>,
    /// The thickness of the border.
    thickness: Animation<i32>,
    /// The color of the border.
    color: Animation<fht_compositor_config::Color>,
}

impl Border {
    /// Create a new [`Border`] for a tile.
    pub fn new(
        geometry: Rectangle<i32, Logical>,
        corner_radius: f32,
        thickness: i32,
        color: fht_compositor_config::Color,
        animation_config: Option<&AnimationConfig>,
    ) -> Self {
        let animation_config = animation_config.unwrap_or(&AnimationConfig::DISABLED);

        Self {
            element: OnceCell::new(),
            geometry,
            // FIXME: This will cause the animation to run in the first place, I kinda want to avoid
            // that. Otherwise this is "fine" (the animations are short-lived anyway)
            thickness: Animation::new(thickness, thickness, animation_config.duration)
                .with_curve(animation_config.curve),
            corner_radius: Animation::new(corner_radius, corner_radius, animation_config.duration)
                .with_curve(animation_config.curve),
            color: Animation::new(color, color, animation_config.duration)
                .with_curve(animation_config.curve),
        }
    }

    /// Update this border's parameters
    pub fn update_parameters(
        &mut self,
        corner_radius: f32,
        thickness: i32,
        color: fht_compositor_config::Color,
    ) {
        let mut changed = false;
        // We only update animations and restart them if they have different targets
        // If we are already moving towards a value no need to update stuff

        if self.corner_radius.end != corner_radius {
            self.corner_radius.start = *self.corner_radius.value();
            self.corner_radius.end = corner_radius;
            self.corner_radius.restart();
            changed = true;
        }

        if self.thickness.end != thickness {
            self.thickness.start = *self.thickness.value();
            self.thickness.end = thickness;
            self.thickness.restart();
            changed = true;
        }

        if self.color.end != color {
            self.color.start = *self.color.value();
            self.color.end = color;
            self.color.restart();
            changed = true;
        }

        if changed {
            self.update_uniforms();
        }
    }

    /// Update the config of this [`Border`]
    pub fn update_config(&mut self, animation_config: Option<&AnimationConfig>) {
        let animation_config = animation_config.unwrap_or(&AnimationConfig::DISABLED);

        self.thickness.set_duration(animation_config.duration);
        self.thickness.set_curve(animation_config.curve);

        self.corner_radius.set_duration(animation_config.duration);
        self.corner_radius.set_curve(animation_config.curve);

        self.color.set_duration(animation_config.duration);
        self.color.set_curve(animation_config.curve);
    }

    /// Advances the animations of this [`Border`], returning `true` if any animations are ongoing
    pub fn advance_animations(&mut self, target_presentation_time: Duration) -> bool {
        let mut ongoing = false;

        if !self.corner_radius.is_finished() {
            self.corner_radius.tick(target_presentation_time);
            ongoing = true
        }

        if !self.thickness.is_finished() {
            self.thickness.tick(target_presentation_time);
            ongoing = true
        }

        if !self.color.is_finished() {
            self.color.tick(target_presentation_time);
            ongoing = true
        }

        if ongoing {
            self.update_uniforms();
        }

        ongoing
    }

    /// Generate uniform values from the current state of this [`Border`]
    fn get_uniforms(&self) -> Vec<Uniform<'static>> {
        let corner_radius = self.corner_radius();
        let thickness = *self.thickness.value();
        let color = *self.color.value();
        let mut uniforms = vec![
            Uniform::new("corner_radius", corner_radius),
            Uniform::new("thickness", thickness as f32),
        ];
        match color {
            fht_compositor_config::Color::Solid(color) => {
                uniforms.push(Uniform::new("color_kind", 0));
                uniforms.push(Uniform::new("color_start", color));
            }
            fht_compositor_config::Color::Gradient { start, end, angle } => {
                uniforms.push(Uniform::new("color_kind", 1));
                uniforms.push(Uniform::new("color_start", start));
                uniforms.push(Uniform::new("color_end", end));
                uniforms.push(Uniform::new("color_angle", angle));
            }
        }

        uniforms
    }

    /// Update the uniform values passed into the [`FhtPixelShaderElement`].
    fn update_uniforms(&mut self) {
        let uniforms = self.get_uniforms();
        if let Some(element) = self.element.get_mut() {
            element.update_uniforms(uniforms);
        }
    }

    /// Get the current corner radius
    pub fn corner_radius(&self) -> f32 {
        let radius = *self.corner_radius.value();
        let size = self.geometry.size;

        // Fit a given border radius value to a size to avoid the corners clipping.
        // SEE: <https://drafts.csswg.org/css-backgrounds/#corner-overlap>
        let reduction = f32::min(size.w as f32 / (2. * radius), size.h as f32 / (2. * radius));
        if reduction < 1.0 {
            radius * reduction
        } else {
            radius
        }
    }

    /// Get the current thickness
    pub fn thickness(&self) -> i32 {
        *self.thickness.value()
    }

    /// Set this border's geometry
    pub fn set_geometry(&mut self, geometry: Rectangle<i32, Logical>) {
        if let Some(element) = self.element.get_mut() {
            element.resize(geometry, None);
        }
    }

    /// Get a render element for this [`Border`]
    pub fn render(&self, renderer: &mut impl FhtRenderer) -> FhtPixelShaderElement {
        self.element
            .get_or_init(|| {
                let program = Shaders::get(renderer.glow_renderer()).border.clone();
                let uniforms = self.get_uniforms();
                FhtPixelShaderElement::new(program, self.geometry, 1.0, uniforms, Kind::Unspecified)
            })
            .clone()
    }
}

// Shadow drawing shader using the following article code:
// https://madebyevan.com/shaders/fast-rounded-rectangle-shadows/
pub fn draw_shadow(
    renderer: &mut impl AsGlowRenderer,
    alpha: f32,
    scale: i32,
    mut geometry: Rectangle<i32, Logical>,
    blur_sigma: f32,
    corner_radius: f32,
    color: [f32; 4],
) -> FhtPixelShaderElement {
    let scaled_blur_sigma = (blur_sigma / scale as f32).round() as i32;
    geometry.loc -= Point::from((scaled_blur_sigma, scaled_blur_sigma));
    geometry.size += Size::from((2 * scaled_blur_sigma, 2 * scaled_blur_sigma));

    FhtPixelShaderElement::new(
        Shaders::get(renderer.glow_renderer()).box_shadow.clone(),
        geometry,
        alpha,
        vec![
            // NOTE: For some reasons we cant use f64s, we shall cast
            Uniform::new("shadow_color", color),
            Uniform::new("blur_sigma", blur_sigma),
            Uniform::new("corner_radius", corner_radius),
        ],
        Kind::Unspecified,
    )
}
