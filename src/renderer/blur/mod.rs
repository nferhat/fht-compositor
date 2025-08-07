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
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::AsRenderElements;
use smithay::backend::renderer::gles::{
    ffi, Capability, GlesError, GlesFrame, GlesRenderer, GlesTexture,
};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::{Bind, Blit, Frame, Offscreen, Renderer, Texture, TextureFilter};
use smithay::desktop::layer_map_for_output;
use smithay::output::Output;
use smithay::reexports::gbm::Format;
use smithay::utils::{Logical, Physical, Point, Rectangle, Size, Transform};
use smithay::wayland::shell::wlr_layer::Layer;

use super::data::RendererData;
use super::shaders::Shaders;
use super::{render_elements, FhtRenderer};
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
        // FIXME: Not panic here?
        let renderer = renderer.glow_renderer_mut();
        let output_size = output.geometry().size;

        fn create_buffer(
            renderer: &mut GlowRenderer,
            size: Size<i32, Logical>,
        ) -> Result<GlesTexture, GlesError> {
            renderer.create_buffer(Format::Abgr8888, size.to_buffer(1, Transform::Normal))
        }

        let this = EffectsFramebuffers {
            optimized_blur: create_buffer(renderer, output_size).unwrap(),
            optimized_blur_dirty: true,
            effects: create_buffer(renderer, output_size).unwrap(),
            effects_swapped: create_buffer(renderer, output_size).unwrap(),
            current_buffer: CurrentBuffer::Normal,
        };

        let user_data = output.user_data();
        assert!(
            user_data.insert_if_missing(|| Rc::new(RefCell::new(this))),
            "EffectsFrambuffers::init_for_output should only be called once!"
        );
    }

    /// Update the [`EffectsFramebuffers`] for an [`Output`].
    ///
    /// You should call this if the output's scale/size changes
    pub fn update_for_output(
        output: &Output,
        renderer: &mut impl FhtRenderer,
    ) -> Result<(), GlesError> {
        let renderer = renderer.glow_renderer_mut();
        let mut fx_buffers = Self::get(output);
        let output_size = output.geometry().size;

        fn create_buffer(
            renderer: &mut GlowRenderer,
            size: Size<i32, Logical>,
        ) -> Result<GlesTexture, GlesError> {
            renderer.create_buffer(Format::Abgr8888, size.to_buffer(1, Transform::Normal))
        }

        *fx_buffers = EffectsFramebuffers {
            optimized_blur: create_buffer(renderer, output_size)?,
            optimized_blur_dirty: true,
            effects: create_buffer(renderer, output_size)?,
            effects_swapped: create_buffer(renderer, output_size)?,
            current_buffer: CurrentBuffer::Normal,
        };

        Ok(())
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
        // NOTE: We use Blur::DISABLED since we should not include blur with Background/Bottom
        // layer shells
        let layer_map = layer_map_for_output(output);

        let mut elements = vec![];
        for layer in layer_map
            .layers_on(Layer::Background)
            .chain(layer_map.layers_on(Layer::Bottom))
            .rev()
        {
            let layer_geo = layer_map.layer_geometry(layer).unwrap();
            let location = layer_geo.loc.to_physical_precise_round(scale);
            elements.extend(layer.render_elements::<WaylandSurfaceRenderElement<_>>(
                renderer,
                location,
                (scale as f64).into(),
                1.0,
            ));
        }
        let mut fb = renderer.bind(&mut self.effects).unwrap();
        let output_rect = output.geometry().to_physical(scale);
        let _ = render_elements(
            renderer,
            &mut fb,
            output_rect.size,
            scale as f64,
            Transform::Normal,
            elements.iter(),
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
            let (sample_buffer, render_buffer) = self.buffers();
            render_blur_pass_with_frame(
                renderer,
                sample_buffer,
                render_buffer,
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
            let (sample_buffer, render_buffer) = self.buffers();
            render_blur_pass_with_frame(
                renderer,
                sample_buffer,
                render_buffer,
                &shaders.up,
                half_pixel,
                config,
            )?;
            self.current_buffer.swap();
        }

        // Now blit from the last render buffer into optimized_blur
        // We are already bound so its just a blit
        let tex_fb = renderer.bind(&mut self.effects).unwrap();
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

    /// Get the sample and render buffers.
    pub fn buffers(&mut self) -> (&GlesTexture, &mut GlesTexture) {
        match self.current_buffer {
            CurrentBuffer::Normal => (&self.effects, &mut self.effects_swapped),
            CurrentBuffer::Swapped => (&self.effects_swapped, &mut self.effects),
        }
    }
}

// Renders a blur pass using a GlesFrame with syncing and fencing provided by smithay. Used for
// updating optimized blur buffer since we are not yet rendering.
fn render_blur_pass_with_frame(
    renderer: &mut GlowRenderer,
    sample_buffer: &GlesTexture,
    render_buffer: &mut GlesTexture,
    program: &shader::BlurShader,
    half_pixel: [f32; 2],
    config: &fht_compositor_config::Blur,
) -> anyhow::Result<()> {
    crate::profile_function!();
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

/// Get a blurred texture for a given region. The region is used as damage.
fn get_blurred_contents(
    frame: &mut GlesFrame,
    output: &Output,
    damage: Rectangle<i32, Physical>,
    scale: i32,
    blur_config: &fht_compositor_config::Blur,
) -> Result<GlesTexture, GlesError> {
    let shaders = Shaders::get_from_frame(frame).blur.clone();
    let vbos = RendererData::get_from_frame(frame).vbos;
    let instancing = frame.capabilities().contains(&Capability::Instancing);
    let projection_matrix = glam::Mat3::from_cols_array(frame.projection());
    let fht_compositor_config::Blur { passes, radius, .. } = *blur_config;

    // We must expand the damage with the blur to damage appropriatly.
    let blur_size = (2f32.powi(passes as i32 + 1) * radius).ceil() as i32 * scale;
    let damage = {
        let mut rect = damage;
        rect.loc -= Point::from((blur_size, blur_size));
        rect.size += Size::from((blur_size, blur_size)).upscale(2);
        rect
    };

    let mut fx_buffers = EffectsFramebuffers::get(output);
    fx_buffers.current_buffer = CurrentBuffer::Normal;

    let blurred_buffer = frame.with_context(move |gl| unsafe {
        // Read previous framebuffer we were bound to
        let mut prev_fbo = 0;
        gl.GetIntegerv(ffi::FRAMEBUFFER_BINDING, &mut prev_fbo as *mut _);

        let (sample_buffer, _) = fx_buffers.buffers();

        // First, we the sample framebuffer contents using a blit.
        //
        // TODO: Avoid using blit by drawing from the render buffer into the texture buffer, this
        // would remove the OpenGL 3.0 dependency for blur to work.
        {
            let mut sample_buffer_fbo = create_texture_fbo(gl, sample_buffer)?;
            gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, sample_buffer_fbo);
            gl.BlitFramebuffer(
                damage.loc.x,
                damage.loc.y,
                damage.loc.x + damage.size.w,
                damage.loc.y + damage.size.h,
                damage.loc.x,
                damage.loc.y,
                damage.loc.x + damage.size.w,
                damage.loc.y + damage.size.h,
                ffi::COLOR_BUFFER_BIT,
                ffi::LINEAR,
            );
            // Dont forget to cleanup the fbo after blitting
            gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, 0);
            gl.DeleteFramebuffers(1, &mut sample_buffer_fbo as _);

            if gl.GetError() == ffi::INVALID_OPERATION {
                error!("TrueBlur needs GLES3.0 for blitting");
                return Err(GlesError::BlitError);
            }
        }

        let tex_size = sample_buffer.size();
        // NOTE: Source and dst are always the same since we are not stretching the texture
        // around, it will always be drawn on top of the whole output.
        let src = Rectangle::from_size(tex_size.to_f64());
        let dest = src
            .to_logical(1.0, Transform::Normal, &src.size)
            .to_physical_precise_round::<_, i32>(scale);
        // NOTE: We are assured that tex_size != 0, and src.size != too (by damage tracker)
        let mut texture_matrix = super::build_texture_mat(src, dest, tex_size, Transform::Normal);
        if sample_buffer.is_y_inverted() {
            texture_matrix *=
                Mat3::from_cols_array(&[1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0]);
        }

        let draw_pass = |render_buffer: &GlesTexture,
                         sample_buffer: &GlesTexture,
                         program: &shader::BlurShader,
                         damage: Rectangle<i32, Physical>,
                         half_pixel: [f32; 2]|
         -> Result<(), GlesError> {
            crate::profile_function!();

            let render_fbo = create_texture_fbo(gl, render_buffer)?;

            // Transform damage into vertices
            let damage = [
                damage.loc.x as f32,
                damage.loc.y as f32,
                damage.size.w as f32,
                damage.size.h as f32,
            ];

            let mut vertices = Vec::with_capacity(4);
            let damage_len = if instancing {
                vertices.extend(damage);
                vertices.len() / 4
            } else {
                for _ in 0..6 {
                    // Add the 4 f32s per damage rectangle for each of the 6 vertices.
                    vertices.extend_from_slice(&damage);
                }

                1
            };

            // Prepare sample texture target
            gl.ActiveTexture(ffi::TEXTURE0);
            gl.BindTexture(ffi::TEXTURE_2D, sample_buffer.tex_id());
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);

            // Setup program and uniforms
            gl.UseProgram(program.program);
            gl.Uniform1f(program.uniform_alpha, 1.0);
            gl.Uniform1f(program.uniform_radius, radius);
            gl.Uniform2f(program.uniform_half_pixel, half_pixel[0], half_pixel[1]);

            gl.Uniform1i(program.uniform_tex, 0);
            gl.UniformMatrix3fv(
                program.uniform_matrix,
                1,
                ffi::FALSE,
                projection_matrix.as_ref() as *const f32,
            );
            gl.UniformMatrix3fv(
                program.uniform_tex_matrix,
                1,
                ffi::FALSE,
                texture_matrix.as_ref() as *const f32,
            );

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

            if instancing {
                gl.VertexAttribDivisor(program.attrib_vert as u32, 0);
                gl.VertexAttribDivisor(program.attrib_vert_position as u32, 1);
                gl.DrawArraysInstanced(ffi::TRIANGLE_STRIP, 0, 4, damage_len as i32);
            } else {
                let count = damage_len * 6;
                gl.DrawArrays(ffi::TRIANGLES, 0, count as i32);
            }

            // Cleanup
            gl.BindTexture(ffi::TEXTURE_2D, 0);
            gl.DisableVertexAttribArray(program.attrib_vert as u32);
            gl.DisableVertexAttribArray(program.attrib_vert_position as u32);
            gl.DeleteFramebuffers(1, &render_fbo as *const _);

            Ok(())
        };

        // Now, draw passes
        let half_pixel = [
            0.5 / (tex_size.w as f32 / 2.0),
            0.5 / (tex_size.h as f32 / 2.0),
        ];

        for i in 0..passes {
            let (sample_buffer, render_buffer) = fx_buffers.buffers();
            draw_pass(
                render_buffer,
                sample_buffer,
                &shaders.down,
                damage.downscale(1 << i),
                half_pixel,
            )?;
            fx_buffers.current_buffer.swap();
        }

        let half_pixel = [
            0.5 / (tex_size.w as f32 * 2.0),
            0.5 / (tex_size.h as f32 * 2.0),
        ];
        for i in 0..passes {
            let (sample_buffer, render_buffer) = fx_buffers.buffers();
            draw_pass(
                render_buffer,
                sample_buffer,
                &shaders.up,
                damage.downscale(1 << passes - i - 1),
                half_pixel,
            )?;
            fx_buffers.current_buffer.swap();
        }

        // Cleanup before returning texture
        gl.BindFramebuffer(ffi::FRAMEBUFFER, prev_fbo as u32);

        Result::<_, GlesError>::Ok(fx_buffers.effects.clone())
    })??;

    Ok(blurred_buffer)
}

/// Create a framebuffer object for a given [`GlesTexture`] used to bind that texture.
unsafe fn create_texture_fbo(gl: &ffi::Gles2, texture: &GlesTexture) -> Result<u32, GlesError> {
    let mut fbo = 0;
    gl.GenFramebuffers(1, &mut fbo as _);
    gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);
    gl.FramebufferTexture2D(
        ffi::FRAMEBUFFER,
        ffi::COLOR_ATTACHMENT0,
        ffi::TEXTURE_2D,
        texture.tex_id(),
        0,
    );
    gl.Clear(ffi::COLOR_BUFFER_BIT);
    let status = gl.CheckFramebufferStatus(ffi::FRAMEBUFFER);
    if status != ffi::FRAMEBUFFER_COMPLETE {
        gl.DeleteFramebuffers(1, &mut fbo as _);
        return Err(GlesError::FramebufferBindingError);
    }

    Ok(fbo)
}
