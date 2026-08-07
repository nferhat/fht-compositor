//! Border rendering for tiles.

use std::cell::RefCell;
use std::time::Duration;

use fht_animation::Animation;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::Uniform;
use smithay::utils::{Logical, Rectangle};

use super::AnimationConfig;
use crate::renderer::shaders::{ShaderElement, Shaders};
use crate::renderer::FhtRenderer;

/// A border around a tile.
///
/// The border grows inside the border geometry.
#[derive(Clone, Debug)]
pub struct Border {
    element: RefCell<Option<ShaderElement>>,
    geometry: Rectangle<i32, Logical>,
    // We store the parameters with the struct
    parameters: Parameters,
    // And we animate each of them below
    radius_animation: Animation<f32>,
    power_animation: Animation<f32>,
    color_animation: Animation<fht_compositor_config::Color>,
}

/// [`Border`] parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parameters {
    /// The border color.
    pub color: fht_compositor_config::Color,
    /// The border radius.
    pub radius: f32,
    /// The border corner radius power.
    pub power: f32,
    /// The border thickness.
    pub thickness: i32,
}

impl Border {
    pub fn new(
        geometry: Rectangle<i32, Logical>,
        parameters: Parameters,
        animation_config: Option<&AnimationConfig>,
    ) -> Self {
        let AnimationConfig { duration, curve } =
            animation_config.unwrap_or(&AnimationConfig::DISABLED);
        let Parameters {
            color,
            radius,
            power,
            ..
        } = parameters;

        Self {
            element: RefCell::new(None), // we initialize the element on the first render call
            geometry,
            parameters,
            // Each animation starts as empty
            radius_animation: Animation::new(radius, radius, *duration).with_curve(*curve),
            power_animation: Animation::new(power, power, *duration).with_curve(*curve),
            color_animation: Animation::new(color, color, *duration).with_curve(*curve),
        }
    }

    /// Advance the animations of this [`Border`], returning whether animations are ongoing or not.
    pub fn advance_animations(&mut self, target_presentation_time: Duration) -> bool {
        let mut ongoing = false;

        if !self.color_animation.is_finished() {
            self.color_animation.tick(target_presentation_time);
            ongoing = true;
        }

        if !self.radius_animation.is_finished() {
            self.radius_animation.tick(target_presentation_time);
            ongoing = true;
        }

        if !self.power_animation.is_finished() {
            self.power_animation.tick(target_presentation_time);
            ongoing = true;
        }

        if ongoing {
            let uniforms = self.uniforms();
            if let Some(element) = self.element.get_mut() {
                element.update_uniforms(uniforms);
            }
        }

        ongoing
    }

    /// Resize this border.
    pub fn set_geometry(&mut self, geometry: Rectangle<i32, Logical>) {
        if self.geometry == geometry {
            return;
        }

        self.geometry = geometry;
        if let Some(element) = self.element.get_mut() {
            element.resize(geometry, None);
        }
    }

    /// Update the border config.
    pub fn update_config(&mut self, animation_config: Option<&AnimationConfig>) {
        let AnimationConfig { duration, curve } =
            animation_config.unwrap_or(&AnimationConfig::DISABLED);
        // FIXME: This does result in some wonky transitions, especially when an animation is
        // happening while the config update (say the user changed both border color and
        // animation config at once)

        self.radius_animation = self
            .radius_animation
            .clone()
            .with_duration(*duration)
            .with_curve(*curve);
        self.color_animation = self
            .color_animation
            .clone()
            .with_duration(*duration)
            .with_curve(*curve);
    }

    /// Update the border parameters
    pub fn update_parameters(&mut self, parameters: Parameters) {
        if self.parameters == parameters {
            return;
        }

        self.parameters = parameters;

        if self.color_animation.end != parameters.color {
            self.color_animation.start = *self.color_animation.value();
            self.color_animation.end = parameters.color;
            self.color_animation.restart();
        }

        if self.radius_animation.end != parameters.radius {
            self.radius_animation.start = *self.radius_animation.value();
            self.radius_animation.end = parameters.radius;
            self.radius_animation.restart();
        }

        if self.power_animation.end != parameters.power {
            self.power_animation.start = *self.power_animation.value();
            self.power_animation.end = parameters.power;
            self.power_animation.restart();
        }

        let uniforms = self.uniforms();
        if let Some(element) = self.element.get_mut() {
            element.update_uniforms(uniforms);
        }
    }

    /// Get the last parameters set by [`Self::udpate_parameters`]
    pub fn parameters(&self) -> &Parameters {
        &self.parameters
    }

    /// Get the currently used parameters, IE the effective values from the animations
    pub fn current_parameters(&self) -> Parameters {
        Parameters {
            color: *self.color_animation.value(),
            radius: *self.radius_animation.value(),
            power: *self.power_animation.value(),
            thickness: self.parameters.thickness,
        }
    }

    fn uniforms(&self) -> Vec<Uniform<'static>> {
        let Parameters {
            color,
            radius,
            power,
            thickness,
        } = self.current_parameters();
        let radius = fit_corner_radius_to_geometry(self.geometry, radius);

        let mut uniforms = vec![
            Uniform::new("corner_radius", radius),
            Uniform::new("corner_power", power),
            Uniform::new("thickness", thickness as f32),
        ];

        match color {
            fht_compositor_config::Color::Solid(solid) => {
                uniforms.extend([
                    Uniform::new("color_kind", 0),
                    Uniform::new("color_start", solid),
                ]);
            }
            fht_compositor_config::Color::Gradient { start, end, angle } => {
                uniforms.extend([
                    Uniform::new("color_kind", 1),
                    Uniform::new("color_start", start),
                    Uniform::new("color_end", end),
                    Uniform::new("color_angle", angle),
                ]);
            }
        }

        uniforms
    }

    /// Get a [`BorderRenderElement`] from this Border
    ///
    /// If returns `None`, the border should not be rendered.
    pub fn render(&self, renderer: &mut impl FhtRenderer, alpha: f32) -> Option<ShaderElement> {
        if self.parameters.thickness == 0 {
            return None;
        }

        let mut guard = self.element.borrow_mut();
        let element = guard.get_or_insert_with(|| {
            let program = Shaders::get(renderer.glow_renderer()).border.clone();
            ShaderElement::new(
                program,
                self.geometry,
                None,
                1.0,
                self.uniforms(),
                Kind::Unspecified,
            )
        });
        element.set_alpha(alpha);

        Some(element.clone())
    }
}

/// Fit a given corner radius to a geometry to avoid corners overlapping and clipping into
/// eachother.
///
/// Formulas from <// https://drafts.csswg.org/css-backgrounds/#corner-overlap>
pub fn fit_corner_radius_to_geometry(geometry: Rectangle<i32, Logical>, corner_radius: f32) -> f32 {
    let reduction = f32::min(
        geometry.size.w as f32 / (2. * corner_radius),
        geometry.size.h as f32 / (2. * corner_radius),
    );
    let reduction = f32::min(1.0, reduction);
    corner_radius * reduction
}
