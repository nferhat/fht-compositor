use fht_compositor_config::Color;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::element::PixelShaderElement;
use smithay::backend::renderer::gles::{GlesPixelProgram, Uniform};
use smithay::utils::{Logical, Rectangle};

use super::pixel_shader_element::FhtPixelShaderElement;
use super::shaders::Shaders;
use super::AsGlowRenderer;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoundedOutlineSettings {
    pub half_thickness: f32,
    pub radius: f32,
    pub color: Color,
}

pub struct RoundedOutlineElement; // this does nothing expect be there.

impl RoundedOutlineElement {
    pub fn program(renderer: &impl AsGlowRenderer) -> GlesPixelProgram {
        Shaders::get(renderer).rounded_outline.clone()
    }

    pub fn element(
        renderer: &mut impl AsGlowRenderer,
        scale: f64,
        alpha: f32,
        geo: Rectangle<i32, Logical>,
        settings: RoundedOutlineSettings,
    ) -> FhtPixelShaderElement {
        let scaled_half_thickness = settings.half_thickness as f32 * scale as f32;
        let program = Self::program(renderer);

        let (start_color, end_color, angle) = match settings.color {
            Color::Solid(color) => (color, color, 0.0),
            Color::Gradient { start, end, angle } => (start, end, angle),
        };
        let mut element = PixelShaderElement::new(
            program,
            geo,
            None,
            alpha,
            vec![
                Uniform::new("v_start_color", start_color),
                Uniform::new("v_end_color", end_color),
                Uniform::new("v_gradient_angle", angle),
                Uniform::new("half_thickness", scaled_half_thickness),
                Uniform::new("radius", settings.radius),
            ],
            Kind::Unspecified,
        );
        element.resize(geo, None);

        FhtPixelShaderElement(element)
    }
}
