use std::borrow::BorrowMut;

use smithay::backend::renderer::gles::{
    GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, UniformName, UniformType,
};
use smithay::backend::renderer::glow::GlowRenderer;

use super::blur::shader::BlurShaders;

const BORDER_SRC: &str = include_str!("./border.frag");
const BOX_SHADOW_SRC: &str = include_str!("./box-shadow.frag");
const ROUNDED_WINDOW_SRC: &str = include_str!("./rounded-window.frag");
const ROUNDED_TEXTURE_SRC: &str = include_str!("./rounded-texture.frag");
const RESIZING_TEXTURE_SRC: &str = include_str!("./resizing-texture.frag");
pub(super) const BLUR_DOWN_SRC: &str = include_str!("./blur-down.frag");
pub(super) const BLUR_UP_SRC: &str = include_str!("./blur-up.frag");
pub(super) const VERTEX_SRC: &str = include_str!("./texture.vert");

pub struct Shaders {
    pub border: GlesPixelProgram,
    pub box_shadow: GlesPixelProgram,
    // rounded_window => complex shader that takes into account subsurface position through
    // matrices, only used in src/space/tile.rs
    pub rounded_window: GlesTexProgram,
    // rounded_texture => simple shader that just rounds off a passed in texture.
    pub rounded_texture: GlesTexProgram,
    pub resizing_texture: GlesTexProgram,
    pub blur: BlurShaders,
}

impl Shaders {
    pub fn init(renderer: &mut GlowRenderer) {
        let renderer: &mut GlesRenderer = renderer.borrow_mut();

        let rounded_window = renderer
            .compile_custom_texture_shader(
                ROUNDED_WINDOW_SRC,
                &[
                    UniformName::new("corner_radius", UniformType::_1f),
                    UniformName::new("geo_size", UniformType::_2f),
                    UniformName::new("input_to_geo", UniformType::Matrix3x3),
                ],
            )
            .expect("Shader source should always compile!");
        let rounded_texture = renderer
            .compile_custom_texture_shader(
                ROUNDED_TEXTURE_SRC,
                &[
                    UniformName::new("corner_radius", UniformType::_1f),
                    UniformName::new("geo", UniformType::_4f),
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
        let border = renderer
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
        let blur = BlurShaders::compile(renderer).expect("Shader source should always compile!");

        let shaders = Self {
            border,
            box_shadow,
            rounded_window,
            rounded_texture,
            resizing_texture,
            blur,
        };

        renderer
            .egl_context()
            .user_data()
            .insert_if_missing(|| shaders);
    }

    pub fn get<'a>(renderer: &'a GlowRenderer) -> &'a Self {
        renderer
            .egl_context()
            .user_data()
            .get()
            .expect("Shaders are initialized at startup!")
    }

    pub fn get_from_frame<'a>(frame: &'a GlesFrame<'_, '_>) -> &'a Self {
        frame
            .egl_context()
            .user_data()
            .get()
            .expect("Shaders are initialized at startup!")
    }
}
