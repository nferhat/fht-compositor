//! Border rendering.
//!
//! This is achieved using a GlesPixelShader, nothing special otherwise.

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::Uniform;
use smithay::utils::{Logical, Rectangle};

use crate::renderer::pixel_shader_element::FhtPixelShaderElement;
use crate::renderer::shaders::Shaders;
use crate::renderer::AsGlowRenderer;

pub fn draw_border(
    renderer: &mut impl AsGlowRenderer,
    scale: f64,
    alpha: f32,
    geometry: Rectangle<i32, Logical>,
    thickness: f64,
    radius: f64,
    color: fht_compositor_config::Color,
) -> FhtPixelShaderElement {
    let scaled_half_thickness = (thickness / 2.0) * scale;
    let (start_color, end_color, angle) = match color {
        fht_compositor_config::Color::Solid(color) => (color, color, 0.0),
        fht_compositor_config::Color::Gradient { start, end, angle } => (start, end, angle),
    };

    FhtPixelShaderElement::new(
        Shaders::get(renderer).border.clone(),
        geometry,
        alpha,
        vec![
            Uniform::new("v_start_color", start_color),
            Uniform::new("v_end_color", end_color),
            Uniform::new("v_gradient_angle", angle),
            // NOTE: For some reasons we cant use f64s, we shall cast
            Uniform::new("half_thickness", scaled_half_thickness as f32),
            Uniform::new("radius", radius as f32),
        ],
        Kind::Unspecified,
    )
}
