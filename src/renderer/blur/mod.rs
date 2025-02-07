//! Blurring algorithm and system integrated into smithay.
//!
//! It is not perfect at the moment but currently I am satisfied enough with how it looks. The
//! actual underlying algorithm is Dual-Kawase, with downscaling then upscaling steps.
//!
//! - <https://github.com/alex47/Dual-Kawase-Blur>
//! - <https://github.com/wlrfx/scenefx>
//! - <https://www.shadertoy.com/view/3td3W8>

pub mod element;
pub(super) mod shader;

use std::borrow::BorrowMut;
use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use anyhow::Context;
use glam::Mat3;
use smithay::backend::renderer::gles::format::fourcc_to_gl_formats;
use smithay::backend::renderer::gles::{ffi, Capability, GlesError, GlesRenderer, GlesTexture};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::{Bind, Blit, Frame, Renderer, Texture, TextureFilter};
use smithay::output::Output;
use smithay::reexports::gbm::Format;
use smithay::utils::{Logical, Physical, Rectangle, Size, Transform};
use smithay::wayland::shell::wlr_layer::Layer;

use super::data::RendererData;
use super::shaders::Shaders;
use super::{layer_elements, render_elements, FhtRenderer};
use crate::output::OutputExt;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum CurrentBuffer {
    /// We are currently sampling from normal buffer, and rendering in the swapped/alternative.
    #[default]
    Normal,
    /// We are currently sampling from swapped buffer, and rendering in the normal.
    Swapped,
}

impl CurrentBuffer {
    pub fn swap(&mut self) {
        *self = match self {
            // sampled from normal, render to swapped
            Self::Normal => Self::Swapped,
            // sampled fro swapped, render to normal next
            Self::Swapped => Self::Normal,
        }
    }
}

/// Effect framebuffers associated with each output.
pub struct EffectsFramebuffers {
    /// Contains the main buffer blurred contents
    pub optimized_blur: GlesTexture,
    /// Whether the optimizer blur buffer is dirty
    pub optimized_blur_dirty: bool,
    // /// Contains the original pixels before blurring to draw with in case of artifacts.
    // blur_saved_pixels: GlesTexture,
    // The blur algorithms (dual-kawase) swaps between these two whenever scaling the image
    effects: GlesTexture,
    effects_swapped: GlesTexture,
    /// The buffer we are currently rendering/sampling from.
    ///
    /// In order todo the up/downscaling, we render into different buffers. On each pass, we render
    /// into a different buffer with downscaling/upscaling (depending on which pass we are at)
    ///
    /// One exception is that if we are on the first pass, we are on [`CurrentBuffer::Initial`], we
    /// are sampling from [`Self::blit_buffer`] from initial screen contents.
    current_buffer: CurrentBuffer,
}

type EffectsFramebufffersUserData = Rc<RefCell<EffectsFramebuffers>>;

impl EffectsFramebuffers {
    /// Get the assiciated [`EffectsFramebuffers`] with this output.
    pub fn get(output: &Output) -> RefMut<'_, Self> {
        let user_data = output
            .user_data()
            .get::<EffectsFramebufffersUserData>()
            .unwrap();
        RefCell::borrow_mut(user_data)
    }

    /// Initialize the [`EffectsFramebuffers`] for an [`Output`].
    ///
    /// The framebuffers handles live inside the Output's user data, use [`Self::get`] to access
    /// them.
    pub fn init_for_output(output: &Output, renderer: &mut impl FhtRenderer) {
        let output_size = output.geometry().size;

        fn create_buffer(renderer: &mut impl FhtRenderer, size: Size<i32, Logical>) -> GlesTexture {
            renderer
                .create_buffer(Format::Abgr8888, size.to_buffer(1, Transform::Normal))
                .expect("gl should always be able to create buffers")
        }

        let this = EffectsFramebuffers {
            optimized_blur: renderer
                .create_buffer(
                    Format::Abgr8888,
                    output_size.to_buffer(1, Transform::Normal),
                )
                .unwrap(),
            optimized_blur_dirty: true,
            // blur_saved_pixels: renderer
            //     .create_buffer(
            //         Format::Abgr8888,
            //         output_size.to_buffer(1, Transform::Normal),
            //     )
            //     .unwrap(),
            effects: create_buffer(renderer, output_size),
            effects_swapped: create_buffer(renderer, output_size),
            current_buffer: CurrentBuffer::Normal,
        };

        let user_data = output.user_data();
        assert!(
            user_data.insert_if_missing(|| Rc::new(RefCell::new(this))),
            "EffectsFrambuffers::init_for_output should only be called once!"
        );
    }

    /// Render the optimized blur buffer again
    pub fn update_optimized_blur_buffer(
        &mut self,
        renderer: &mut GlowRenderer,
        output: &Output,
        scale: i32,
        config: &fht_compositor_config::Blur,
    ) -> anyhow::Result<()> {
        // first render layer shell elements
        let elements = layer_elements(renderer, output, Layer::Background)
            .into_iter()
            .chain(layer_elements(renderer, output, Layer::Bottom));
        let mut fb = renderer.bind(&mut self.effects).unwrap();
        let output_rect = output.geometry().to_physical(scale);
        let _ = render_elements(
            renderer,
            &mut fb,
            output_rect.size,
            scale as f64,
            Transform::Normal,
            elements,
        )
        .expect("failed to render for optimized blur buffer");
        drop(fb);
        self.current_buffer = CurrentBuffer::Normal;

        let shaders = Shaders::get(renderer).blur.clone();

        // NOTE: If we only do one pass its kinda ugly, there must be at least
        // n=2 passes in order to have good sampling
        let half_pixel = [
            0.5 / (output_rect.size.w as f32 / 2.0),
            0.5 / (output_rect.size.h as f32 / 2.0),
        ];
        for _ in 0..config.passes {
            let mut render_buffer = self.render_buffer();
            let sample_buffer = self.sample_buffer();
            render_blur_pass_with_frame(
                renderer,
                &sample_buffer,
                &mut render_buffer,
                &shaders.down,
                half_pixel,
                config,
            )?;
            self.current_buffer.swap();
        }

        let half_pixel = [
            0.5 / (output_rect.size.w as f32 * 2.0),
            0.5 / (output_rect.size.h as f32 * 2.0),
        ];
        // FIXME: Why we need inclusive here but down is exclusive?
        for _ in 0..config.passes {
            let mut render_buffer = self.render_buffer();
            let sample_buffer = self.sample_buffer();
            render_blur_pass_with_frame(
                renderer,
                &sample_buffer,
                &mut render_buffer,
                &shaders.up,
                half_pixel,
                config,
            )?;
            self.current_buffer.swap();
        }

        // Now blit from the last render buffer into optimized_blur
        // We are already bound so its just a blit
        let mut target_texture = self.sample_buffer();
        let tex_fb = renderer.bind(&mut target_texture).unwrap();
        let mut optimized_blur_fb = renderer.bind(&mut self.optimized_blur).unwrap();

        renderer.blit(
            &tex_fb,
            &mut optimized_blur_fb,
            Rectangle::from_size(output_rect.size),
            Rectangle::from_size(output_rect.size),
            TextureFilter::Linear,
        )?;

        Ok(())
    }

    /// Get the buffer that was sampled from in the previous pass.
    pub fn sample_buffer(&self) -> GlesTexture {
        match self.current_buffer {
            CurrentBuffer::Normal => self.effects.clone(),
            CurrentBuffer::Swapped => self.effects_swapped.clone(),
        }
    }

    /// Get the buffer that was rendered into in the previous pass.
    pub fn render_buffer(&self) -> GlesTexture {
        match self.current_buffer {
            CurrentBuffer::Normal => self.effects_swapped.clone(),
            CurrentBuffer::Swapped => self.effects.clone(),
        }
    }
}

// Renders a blur pass using a GlesFrame with syncing and fencing provided by smithay. Used for
// updating optimized blur buffer since we are not yet rendering.
fn render_blur_pass_with_frame(
    renderer: &mut GlowRenderer,
    sample_buffer: &GlesTexture,
    render_buffer: &mut GlesTexture,
    blur_program: &shader::BlurShader,
    half_pixel: [f32; 2],
    config: &fht_compositor_config::Blur,
) -> anyhow::Result<()> {
    // We use a texture render element with a custom GlesTexProgram in order todo the blurring
    // At least this is what swayfx/scenefx do, but they just use gl calls directly.
    let size = sample_buffer.size().to_logical(1, Transform::Normal);

    let vbos = RendererData::get(renderer.borrow_mut()).vbos;
    let is_shared = renderer.egl_context().is_shared();

    let mut fb = renderer.bind(render_buffer)?;
    // Using GlesFrame since I want to use a custom program
    let renderer: &mut GlesRenderer = renderer.borrow_mut();
    let mut frame = renderer
        .render(&mut fb, size.to_physical(1), Transform::Normal)
        .context("failed to create frame")?;

    let supports_instaning = frame.capabilities().contains(&Capability::Instancing);
    let debug = !frame.debug_flags().is_empty();
    let projection = Mat3::from_cols_array(frame.projection());

    let tex_size = sample_buffer.size();
    let src = Rectangle::from_size(sample_buffer.size()).to_f64();
    let dst = Rectangle::from_size(size).to_physical(1);

    frame.with_context(|gl| unsafe {
        // We are doing basically what Frame::render_texture_from_to does, but our own shader struct
        // instead. This allows me to get into the gl plumbing.

        // NOTE: We are rendering at the origin of the texture, no need to translate
        let mut mat = Mat3::IDENTITY;
        let src_size = sample_buffer.size().to_f64();

        if tex_size.is_empty() || src_size.is_empty() {
            return Ok(());
        }

        let mut tex_mat = super::build_texture_mat(src, dst, tex_size, Transform::Normal);
        if sample_buffer.is_y_inverted() {
            tex_mat *= Mat3::from_cols_array(&[1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0]);
        }

        // NOTE: We know that this texture is always opaque so skip on some logic checks and
        // directly render. The following code is from GlesRenderer::render_texture
        gl.Disable(ffi::BLEND);

        // Since we are just rendering onto the offsreen buffer, the vertices to draw are only 4
        let damage = [
            dst.loc.x as f32,
            dst.loc.y as f32,
            dst.size.w as f32,
            dst.size.h as f32,
        ];

        let mut vertices = Vec::with_capacity(4);
        let damage_len = if supports_instaning {
            vertices.extend(damage);
            vertices.len() / 4
        } else {
            for _ in 0..6 {
                // Add the 4 f32s per damage rectangle for each of the 6 vertices.
                vertices.extend_from_slice(&damage);
            }

            1
        };

        mat *= projection;

        // SAFETY: internal texture should always have a format
        // We also use Abgr8888 which is known and confirmed
        let (internal_format, _, _) =
            fourcc_to_gl_formats(sample_buffer.format().unwrap()).unwrap();
        let variant = blur_program.variant_for_format(Some(internal_format), false);

        let program = if debug {
            &variant.debug
        } else {
            &variant.normal
        };

        gl.ActiveTexture(ffi::TEXTURE0);
        gl.BindTexture(ffi::TEXTURE_2D, sample_buffer.tex_id());
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
        gl.UseProgram(program.program);

        gl.Uniform1i(program.uniform_tex, 0);
        gl.UniformMatrix3fv(
            program.uniform_matrix,
            1,
            ffi::FALSE,
            mat.as_ref() as *const f32,
        );
        gl.UniformMatrix3fv(
            program.uniform_tex_matrix,
            1,
            ffi::FALSE,
            tex_mat.as_ref() as *const f32,
        );
        gl.Uniform1f(program.uniform_alpha, 1.0);
        gl.Uniform1f(program.uniform_radius, config.radius);
        gl.Uniform2f(program.uniform_half_pixel, half_pixel[0], half_pixel[1]);

        gl.EnableVertexAttribArray(program.attrib_vert as u32);
        gl.BindBuffer(ffi::ARRAY_BUFFER, vbos[0]);
        gl.VertexAttribPointer(
            program.attrib_vert as u32,
            2,
            ffi::FLOAT,
            ffi::FALSE,
            0,
            std::ptr::null(),
        );

        // vert_position
        gl.EnableVertexAttribArray(program.attrib_vert_position as u32);
        gl.BindBuffer(ffi::ARRAY_BUFFER, 0);

        gl.VertexAttribPointer(
            program.attrib_vert_position as u32,
            4,
            ffi::FLOAT,
            ffi::FALSE,
            0,
            vertices.as_ptr() as *const _,
        );

        if supports_instaning {
            gl.VertexAttribDivisor(program.attrib_vert as u32, 0);
            gl.VertexAttribDivisor(program.attrib_vert_position as u32, 1);
            gl.DrawArraysInstanced(ffi::TRIANGLE_STRIP, 0, 4, damage_len as i32);
        } else {
            let count = damage_len * 6;
            gl.DrawArrays(ffi::TRIANGLES, 0, count as i32);
        }

        gl.BindTexture(ffi::TEXTURE_2D, 0);
        gl.DisableVertexAttribArray(program.attrib_vert as u32);
        gl.DisableVertexAttribArray(program.attrib_vert_position as u32);

        gl.Enable(ffi::BLEND);
        gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);

        // FIXME: Check for Fencing support
        if is_shared {
            gl.Finish();
        }

        Result::<_, GlesError>::Ok(())
    })??;

    let _sync_point = frame.finish()?;

    Ok(())
}

// Renders a blur pass using gl code bypassing smithay's Frame mechanisms
//
// When rendering blur in real-time (for windows, for example) there should not be a wait for
// fencing/finishing since this will be done when sending the fb to the output. Using a Frame
// forces us to do that.
unsafe fn render_blur_pass_with_gl(
    gl: &ffi::Gles2,
    vbos: &[u32; 2],
    debug: bool,
    supports_instancing: bool,
    projection_matrix: Mat3,
    // The buffers used for blurring
    sample_buffer: &GlesTexture,
    render_buffer: &mut GlesTexture,
    scale: i32,
    // The current blur program + config
    blur_program: &shader::BlurShader,
    half_pixel: [f32; 2],
    config: &fht_compositor_config::Blur,
    // dst is the region that should have blur
    // it gets up/downscaled with passes
    damage: Rectangle<i32, Physical>,
) -> Result<(), GlesError> {
    let tex_size = sample_buffer.size();
    let src = Rectangle::from_size(tex_size.to_f64());
    let dest = src
        .to_logical(1.0, Transform::Normal, &src.size)
        .to_physical(scale as f64)
        .to_i32_round();

    // FIXME: Should we call gl.Finish() when done rendering this pass? If yes, should we check
    // if the gl context is shared or not? What about fencing, we don't have access to that

    // PERF: Instead of taking the whole src/dst as damage, adapt the code to run with only the
    // damaged window? This would cause us to make a custom WaylandSurfaceRenderElement to blur out
    // stuff. Complicated.

    // First bind to our render buffer
    let mut render_buffer_fbo = 0;
    {
        gl.GenFramebuffers(1, &mut render_buffer_fbo as *mut _);
        gl.BindFramebuffer(ffi::FRAMEBUFFER, render_buffer_fbo);
        gl.FramebufferTexture2D(
            ffi::FRAMEBUFFER,
            ffi::COLOR_ATTACHMENT0,
            ffi::TEXTURE_2D,
            render_buffer.tex_id(),
            0,
        );

        let status = gl.CheckFramebufferStatus(ffi::FRAMEBUFFER);
        if status != ffi::FRAMEBUFFER_COMPLETE {
            return Err(GlesError::FramebufferBindingError);
        }
    }

    let mat = projection_matrix;
    // NOTE: We are assured that tex_size != 0, and src.size != too (by damage tracker)
    let mut tex_mat = super::build_texture_mat(src, dest, tex_size, Transform::Normal);
    if sample_buffer.is_y_inverted() {
        tex_mat *= Mat3::from_cols_array(&[1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0]);
    }

    gl.Disable(ffi::BLEND);

    // FIXME: Use actual damage for this? Would require making a custom window render element that
    // includes blur and whatnot to get the damage for the window only
    let damage = [
        damage.loc.x as f32,
        damage.loc.y as f32,
        damage.size.w as f32,
        damage.size.h as f32,
    ];

    let mut vertices = Vec::with_capacity(4);
    let damage_len = if supports_instancing {
        vertices.extend(damage);
        vertices.len() / 4
    } else {
        for _ in 0..6 {
            // Add the 4 f32s per damage rectangle for each of the 6 vertices.
            vertices.extend_from_slice(&damage);
        }

        1
    };

    // SAFETY: internal texture should always have a format
    // We also use Abgr8888 which is known and confirmed
    let (internal_format, _, _) = fourcc_to_gl_formats(sample_buffer.format().unwrap()).unwrap();
    let variant = blur_program.variant_for_format(Some(internal_format), false);

    let program = if debug {
        &variant.debug
    } else {
        &variant.normal
    };

    gl.ActiveTexture(ffi::TEXTURE0);
    gl.BindTexture(ffi::TEXTURE_2D, sample_buffer.tex_id());
    gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
    gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
    gl.UseProgram(program.program);

    gl.Uniform1i(program.uniform_tex, 0);
    gl.UniformMatrix3fv(
        program.uniform_matrix,
        1,
        ffi::FALSE,
        mat.as_ref() as *const f32,
    );
    gl.UniformMatrix3fv(
        program.uniform_tex_matrix,
        1,
        ffi::FALSE,
        tex_mat.as_ref() as *const f32,
    );
    gl.Uniform1f(program.uniform_alpha, 1.0);
    gl.Uniform1f(program.uniform_radius, config.radius);
    gl.Uniform2f(program.uniform_half_pixel, half_pixel[0], half_pixel[1]);

    gl.EnableVertexAttribArray(program.attrib_vert as u32);
    gl.BindBuffer(ffi::ARRAY_BUFFER, vbos[0]);
    gl.VertexAttribPointer(
        program.attrib_vert as u32,
        2,
        ffi::FLOAT,
        ffi::FALSE,
        0,
        std::ptr::null(),
    );

    // vert_position
    gl.EnableVertexAttribArray(program.attrib_vert_position as u32);
    gl.BindBuffer(ffi::ARRAY_BUFFER, 0);

    gl.VertexAttribPointer(
        program.attrib_vert_position as u32,
        4,
        ffi::FLOAT,
        ffi::FALSE,
        0,
        vertices.as_ptr() as *const _,
    );

    if supports_instancing {
        gl.VertexAttribDivisor(program.attrib_vert as u32, 0);
        gl.VertexAttribDivisor(program.attrib_vert_position as u32, 1);
        gl.DrawArraysInstanced(ffi::TRIANGLE_STRIP, 0, 4, damage_len as i32);
    } else {
        let count = damage_len * 6;
        gl.DrawArrays(ffi::TRIANGLES, 0, count as i32);
    }

    gl.BindTexture(ffi::TEXTURE_2D, 0);
    gl.DisableVertexAttribArray(program.attrib_vert as u32);
    gl.DisableVertexAttribArray(program.attrib_vert_position as u32);

    // Clean up
    {
        gl.Enable(ffi::BLEND);
        gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
        gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
    }

    Ok(())
}
