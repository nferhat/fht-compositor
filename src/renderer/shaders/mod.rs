use std::borrow::{Borrow, BorrowMut};

use smithay::backend::renderer::gles::{
    GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, UniformName, UniformType,
};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};

use super::{AsGlowFrame, AsGlowRenderer};

const BORDER_SRC: &str = include_str!("./border.frag");
const BOX_SHADOW_SRC: &str = include_str!("./box-shadow.frag");
const ROUNDED_QUAD_SRC: &str = include_str!("../rounded_element/shader.frag");
const RESIZING_TEXTURE_SRC: &str = include_str!("./resizing-texture.frag");
const BLUR_DOWN_SRC: &str = include_str!("./blur-down.frag");
const BLUR_UP_SRC: &str = include_str!("./blur-up.frag");

pub struct Shaders {
    pub border: GlesPixelProgram,
    pub box_shadow: GlesPixelProgram,
    pub rounded_quad: GlesTexProgram,
    pub resizing_texture: GlesTexProgram,
    pub blur_down: GlesTexProgram,
    pub blur_up: GlesTexProgram,
}

impl Shaders {
    pub fn init(renderer: &mut GlowRenderer) {
        let renderer: &mut GlesRenderer = renderer.borrow_mut();

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
        let resizing_texture = renderer
            .compile_custom_texture_shader(
                RESIZING_TEXTURE_SRC,
                &[
                    UniformName::new("corner_radius", UniformType::_1f),
                    // the size of the window texture we sampled from
                    UniformName::new("win_size", UniformType::_2f),
                    UniformName::new("curr_size", UniformType::_2f),
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
                    UniformName::new("corner_radius", UniformType::_1f),
                    UniformName::new("thickness", UniformType::_1f),
                ],
            )
            .expect("Shader source should always compile!");
        let box_shadow = renderer
            .compile_custom_pixel_shader(
                BOX_SHADOW_SRC,
                &[
                    UniformName::new("shadow_color", UniformType::_4f),
                    UniformName::new("corner_radius", UniformType::_1f),
                    UniformName::new("blur_sigma", UniformType::_1f),
                ],
            )
            .expect("Shader source should always compile!");
        let blur_down = renderer
            .compile_custom_texture_shader(
                BLUR_DOWN_SRC,
                &[
                    UniformName::new("radius", UniformType::_1f),
                    UniformName::new("half_pixel", UniformType::_2f),
                ],
            )
            .expect("Shader source should always compile");
        let blur_up = renderer
            .compile_custom_texture_shader(
                BLUR_UP_SRC,
                &[
                    UniformName::new("radius", UniformType::_1f),
                    UniformName::new("half_pixel", UniformType::_2f),
                ],
            )
            .expect("Shader source should always compile");

        let shaders = Self {
            border: rounded_outline,
            box_shadow,
            rounded_quad,
            resizing_texture,
            blur_down,
            blur_up,
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
