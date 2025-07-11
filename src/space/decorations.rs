//! Decorations rendering.
//!
//! This is achieved using a GlesPixelShader, nothing special otherwise.

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::Uniform;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::renderer::shaders::{ShaderElement, Shaders};
use crate::renderer::AsGlowRenderer;

pub fn draw_border(
    renderer: &mut impl AsGlowRenderer,
    scale: i32,
    alpha: f32,
    geometry: Rectangle<i32, Logical>,
    thickness: f64,
    radius: f64,
    color: fht_compositor_config::Color,
) -> ShaderElement {
    let scaled_thickness = thickness * scale as f64;
    let (start_color, end_color, angle) = match color {
        fht_compositor_config::Color::Solid(color) => (color, color, 0.0),
        fht_compositor_config::Color::Gradient { start, end, angle } => (start, end, angle),
    };

    ShaderElement::new(
        Shaders::get(renderer.glow_renderer()).border.clone(),
        geometry,
        None,
        alpha,
        vec![
            Uniform::new("v_start_color", start_color),
            Uniform::new("v_end_color", end_color),
            Uniform::new("v_gradient_angle", angle),
            // NOTE: For some reasons we cant use f64s, we shall cast
            Uniform::new("thickness", scaled_thickness as f32),
            Uniform::new("corner_radius", radius as f32),
        ],
        Kind::Unspecified,
    )
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
) -> ShaderElement {
    let scaled_blur_sigma = (blur_sigma / scale as f32).round() as i32;
    geometry.loc -= Point::from((scaled_blur_sigma, scaled_blur_sigma));
    geometry.size += Size::from((2 * scaled_blur_sigma, 2 * scaled_blur_sigma));

    ShaderElement::new(
        Shaders::get(renderer.glow_renderer()).box_shadow.clone(),
        geometry,
        None,
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
