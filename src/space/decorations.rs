//! Decorations rendering.
//!
//! This is achieved using a GlesPixelShader, nothing special otherwise.

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::Uniform;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::renderer::shaders::{ShaderElement, Shaders};
use crate::renderer::AsGlowRenderer;

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
