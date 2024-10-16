use std::borrow::{Borrow, BorrowMut};

use smithay::backend::renderer::gles::{
    GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, UniformName, UniformType,
};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};

use super::{AsGlowFrame, AsGlowRenderer};

const BORDER_SRC: &str = include_str!("./border.frag");
const ROUNDED_QUAD_SRC: &str = include_str!("../rounded_element/shader.frag");

pub struct Shaders {
    pub border: GlesPixelProgram,
    pub rounded_quad: GlesTexProgram,
}

impl Shaders {
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
                BORDER_SRC,
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
            border: rounded_outline,
            rounded_quad,
        };

        renderer
            .egl_context()
            .user_data()
            .insert_if_missing(|| shaders);
    }

    pub fn get<'a>(renderer: &'a impl AsGlowRenderer) -> &'a Self {
        renderer
            .glow_renderer()
            .egl_context()
            .user_data()
            .get()
            .expect("Shaders are initialized at startup!")
    }

    pub fn get_from_frame<'a>(frame: &'a GlowFrame<'_>) -> &'a Self {
        Borrow::<GlesFrame>::borrow(frame.glow_frame())
            .egl_context()
            .user_data()
            .get()
            .expect("Shaders are initialized at startup!")
    }
}
