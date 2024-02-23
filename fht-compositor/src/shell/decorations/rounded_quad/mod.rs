use std::borrow::BorrowMut;

use smithay::backend::egl::EGLContext;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexProgram, UniformName, UniformType};

use crate::backend::render::AsGlowRenderer;

pub struct RoundedQuadShader(pub GlesTexProgram);

impl RoundedQuadShader {
    const SRC: &'static str = include_str!("./shader.frag");

    /// Initialize the shader for the given renderer.
    ///
    /// The shader is stored inside the renderer's EGLContext user data.
    pub fn init(renderer: &mut impl AsGlowRenderer) {
        let renderer = BorrowMut::<GlesRenderer>::borrow_mut(renderer.glow_renderer_mut());

        let program = renderer
            .compile_custom_texture_shader(
                Self::SRC,
                &[
                    UniformName::new("radius", UniformType::_1f),
                    // since smithay doesn't pass it to texture shaders
                    UniformName::new("size", UniformType::_2f),
                ],
            )
            .expect("Failed to compile rounded outline shader!");
        renderer
            .egl_context()
            .user_data()
            .insert_if_missing(|| RoundedQuadShader(program));
    }

    /// Get a reference to the shader instance stored in this renderer EGLContext userdata.
    ///
    /// If you didn't initialize the shader before, this function will do it for you.
    pub fn get(egl_context: &EGLContext) -> GlesTexProgram {
        egl_context
            .user_data()
            .get::<RoundedQuadShader>()
            .expect("Shaders didn't initialize!")
            .0
            .clone()
    }
}
