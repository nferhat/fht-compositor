mod element;
use std::borrow::BorrowMut;

pub use element::ShaderElement;
use smithay::backend::renderer::gles::{
    GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, UniformName, UniformType,
};
use smithay::backend::renderer::glow::GlowRenderer;

use crate::renderer::blur::BlurProgram;

const BORDER_SRC: &str = include_str!("./border.frag");
const BOX_SHADOW_SRC: &str = include_str!("./box-shadow.frag");
const ROUNDED_WINDOW_SRC: &str = include_str!("./rounded-window.frag");
const RESIZING_TEXTURE_SRC: &str = include_str!("./resizing-texture.frag");
const BLUR_POST_SRC: &str = include_str!("./blur-post.frag");
const ROUNDED_CORNERS_SRC: &str = include_str!("./rounded-corners.glsl");

/// Preprocess shaders to handle includes.
fn preprocess_shader_source(source: &str) -> String {
    let mut ret = source.to_string();
    const INCLUDES: &[(&str, &str)] = &[("rounded-corners.glsl", ROUNDED_CORNERS_SRC)];
    for (file_path, replace_with) in INCLUDES {
        ret = ret.replace(&format!(r#"#include "{file_path}""#), replace_with);
    }
    ret
}

pub struct Shaders {
    pub border: GlesPixelProgram,
    pub box_shadow: GlesPixelProgram,
    // rounded_window => complex shader that takes into account subsurface position through
    // matrices, only used in src/space/tile.rs
    pub rounded_window: GlesTexProgram,
    pub resizing_texture: GlesTexProgram,
    pub blur_post: GlesTexProgram,
    pub blur: BlurProgram,
}

impl Shaders {
    pub fn init(renderer: &mut GlowRenderer) {
        let renderer: &mut GlesRenderer = renderer.borrow_mut();

        let rounded_window = renderer
            .compile_custom_texture_shader(
                preprocess_shader_source(ROUNDED_WINDOW_SRC),
                &[
                    UniformName::new("corner_radius", UniformType::_1f),
                    UniformName::new("corner_power", UniformType::_1f),
                    UniformName::new("geo_size", UniformType::_2f),
                    UniformName::new("input_to_geo", UniformType::Matrix3x3),
                ],
            )
            .expect("Shader source should always compile!");

        let resizing_texture = renderer
            .compile_custom_texture_shader(
                preprocess_shader_source(RESIZING_TEXTURE_SRC),
                &[
                    UniformName::new("corner_radius", UniformType::_1f),
                    UniformName::new("corner_power", UniformType::_1f),
                    // the size of the window texture we sampled from
                    UniformName::new("win_size", UniformType::_2f),
                    UniformName::new("curr_size", UniformType::_2f),
                ],
            )
            .expect("Shader source should always compile!");
        let border = renderer
            .compile_custom_pixel_shader(
                preprocess_shader_source(BORDER_SRC),
                &[
                    UniformName::new("color_start", UniformType::_4f),
                    UniformName::new("color_end", UniformType::_4f),
                    UniformName::new("color_angle", UniformType::_1f),
                    UniformName::new("color_kind", UniformType::_1i),
                    UniformName::new("corner_radius", UniformType::_1f),
                    UniformName::new("corner_power", UniformType::_1f),
                    UniformName::new("thickness", UniformType::_1f),
                ],
            )
            .expect("Shader source should always compile!");
        let box_shadow = renderer
            .compile_custom_pixel_shader(
                preprocess_shader_source(BOX_SHADOW_SRC),
                &[
                    UniformName::new("shadow_color", UniformType::_4f),
                    UniformName::new("corner_radius", UniformType::_1f),
                    UniformName::new("corner_power", UniformType::_1f),
                    UniformName::new("blur_sigma", UniformType::_1f),
                ],
            )
            .expect("Shader source should always compile!");

        let blur_post = renderer
            .compile_custom_texture_shader(
                preprocess_shader_source(BLUR_POST_SRC),
                &[
                    UniformName::new("geo_size", UniformType::_2f),
                    UniformName::new("rect_origin", UniformType::_2f),
                    UniformName::new("rect_size", UniformType::_2f),
                    UniformName::new("corner_radius", UniformType::_1f),
                    UniformName::new("corner_power", UniformType::_1f),
                    // blur kind: liquid glass
                    UniformName::new("glass_enabled", UniformType::_1f),
                    UniformName::new("edge_thickness", UniformType::_1f),
                    UniformName::new("refraction_strength", UniformType::_1f),
                    UniformName::new("lens_distortion", UniformType::_1f),
                    UniformName::new("chromatic_aberration", UniformType::_1f),
                    UniformName::new("fresnel_strength", UniformType::_1f),
                    UniformName::new("specular_strength", UniformType::_1f),
                    UniformName::new("glass_opacity", UniformType::_1f),
                    UniformName::new("brightness", UniformType::_1f),
                    UniformName::new("tint_color", UniformType::_4f),
                    // post processing unrelated to blur kind
                    UniformName::new("contrast", UniformType::_1f),
                    UniformName::new("saturation", UniformType::_1f),
                    UniformName::new("vibrancy", UniformType::_1f),
                    UniformName::new("dark_vibrancy", UniformType::_1f),
                ],
            )
            .expect("Shader source should always compile!");

        let blur = BlurProgram::compile(renderer).expect("Shader source should always compile!");

        let shaders = Self {
            border,
            box_shadow,
            rounded_window,
            resizing_texture,
            blur_post,
            blur,
        };

        renderer
            .egl_context()
            .user_data()
            .insert_if_missing(|| shaders);
    }

    pub fn get(renderer: &GlowRenderer) -> &Self {
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
