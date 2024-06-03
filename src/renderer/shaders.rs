use std::borrow::{Borrow, BorrowMut};

use smithay::backend::renderer::gles::{
    GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, UniformName, UniformType,
};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};

use super::{AsGlowFrame, AsGlowRenderer};

const ROUNDED_OUTLINE_SRC: &str = include_str!("./rounded_outline_shader/shader.frag");
const ROUNDED_QUAD_SRC: &str = include_str!("./rounded_element/shader.frag");

/// The shaders a renderer can store.
pub struct Shaders {
    /// Shader used to render a rounded outline with a given radius, color, and size.
    pub rounded_outline: GlesPixelProgram,
    /// Shader used to clip a given render element to a rounded quad.
    pub rounded_quad: GlesTexProgram,
}

impl Shaders {
    /// Initialize all the shaders for a given renderer.
    pub fn init(renderer: &mut GlowRenderer) {
        let renderer: &mut GlesRenderer = renderer.borrow_mut();

        // Rounded outline and rounded quad are included with the compositor, no issues should
        // arise here. (hopefully)
        let rounded_quad = renderer
            .compile_custom_texture_shader(
                ROUNDED_QUAD_SRC,
                &[
                    UniformName::new("corner_radius", UniformType::_1f),
                    UniformName::new("geo_size", UniformType::_2f),
                    UniformName::new("input_to_geo", UniformType::Matrix3x3),
                ],
            )
            .expect("Shader source should always compile!");
        let rounded_outline = renderer
            .compile_custom_pixel_shader(
                ROUNDED_OUTLINE_SRC,
                &[
                    UniformName::new("v_start_color", UniformType::_4f),
                    UniformName::new("v_end_color", UniformType::_4f),
                    UniformName::new("v_gradient_angle", UniformType::_1f),
                    UniformName::new("radius", UniformType::_1f),
                    UniformName::new("half_thickness", UniformType::_1f),
                ],
            )
            .expect("Shader source should always compile!");

        let shaders = Self {
            rounded_outline,
            rounded_quad,
        };

        renderer
            .egl_context()
            .user_data()
            .insert_if_missing(|| shaders);
    }

    /// Get the shaders.
    pub fn get<'a>(renderer: &'a impl AsGlowRenderer) -> &'a Self {
        renderer
            .glow_renderer()
            .egl_context()
            .user_data()
            .get()
            .expect("Shaders are initialized at startup!")
    }

    /// Get the shaders from a frame.
    pub fn get_from_frame<'a>(frame: &'a GlowFrame<'_>) -> &'a Self {
        Borrow::<GlesFrame>::borrow(frame.glow_frame())
            .egl_context()
            .user_data()
            .get()
            .expect("Shaders are initialized at startup!")
    }
}
