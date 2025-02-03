use std::borrow::BorrowMut;

use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    ffi, GlesError, GlesFrame, GlesRenderer, GlesTexture, Uniform,
};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::output::Output;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use super::{CurrentBuffer, EffectsFramebuffers};
#[cfg(feature = "udev-backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};
use crate::output::OutputExt;
use crate::renderer::data::RendererData;
use crate::renderer::shaders::Shaders;
use crate::renderer::texture_element::FhtTextureElement;
use crate::renderer::FhtRenderer;

#[derive(Debug)]
pub enum BlurElement {
    /// Use optimized blur, aka X-ray blur.
    ///
    /// This technique relies on [`EffectsFramebuffers::optimized_blur`] to be populated. It will
    /// render this texture no matter what is below the blur render element.
    Optimized {
        tex: FhtTextureElement,
        corner_radius: f32,
    },
    /// Use true blur.
    ///
    /// When using this technique, the compositor will blur the current framebuffer ccontents that
    /// are below the [`BlurElement`] in order to display them. This adds an additional render step
    /// but provides true results with the blurred contents.
    TrueBlur {
        // we are just a funny texture element that generates the texture on RenderElement::draw
        id: Id,
        scale: i32,
        transform: Transform,
        alpha: f32,
        src: Rectangle<f64, Logical>,
        size: Size<i32, Logical>,
        corner_radius: f32,
        loc: Point<i32, Physical>,
        output: Output,
        // FIXME: Use DamageBag and expand it as needed?
        commit_counter: CommitCounter,
    },
}

impl BlurElement {
    /// Create a new [`BlurElement`]. You are supposed to put this **below** the translucent surface
    /// that you want to blur. `area` is assumed to be relative to the `output` you are rendering
    /// in.
    ///
    /// If you don't update the blur optimized buffer
    /// [`EffectsFramebuffers::update_optimized_blur_buffer`] this element will either
    /// - Display outdated/wrong contents
    /// - Not display anything since the buffer will be empty.
    pub fn new(
        renderer: &mut impl FhtRenderer,
        output: &Output,
        sample_area: Rectangle<i32, Logical>,
        loc: Point<i32, Physical>,
        corner_radius: f32,
        optimized: bool,
        scale: i32,
    ) -> Self {
        let fbs = &mut *EffectsFramebuffers::get(output);
        let texture = fbs.optimized_blur.clone();

        if optimized {
            let texture = TextureRenderElement::from_static_texture(
                Id::new(),
                renderer.id(),
                loc.to_f64(),
                texture,
                scale,
                Transform::Normal,
                Some(1.0),
                Some(sample_area.to_f64()),
                Some(sample_area.size),
                // NOTE: Since this is "optimized" blur, anything below the window will not be
                // rendered
                Some(vec![sample_area.to_buffer(
                    scale,
                    Transform::Normal,
                    &sample_area.size,
                )]),
                Kind::Unspecified,
            );

            Self::Optimized {
                tex: texture.into(),
                corner_radius,
            }
        } else {
            Self::TrueBlur {
                id: Id::new(),
                scale,
                src: sample_area.to_f64(),
                transform: Transform::Normal,
                alpha: 1.0,
                size: sample_area.size,
                corner_radius,
                loc,
                output: output.clone(), // fixme i hate this
                commit_counter: CommitCounter::default(),
            }
        }
    }
}

impl Element for BlurElement {
    fn id(&self) -> &Id {
        match self {
            BlurElement::Optimized { tex, .. } => tex.id(),
            BlurElement::TrueBlur { id, .. } => id,
        }
    }

    fn current_commit(&self) -> CommitCounter {
        match self {
            BlurElement::Optimized { tex, .. } => tex.current_commit(),
            BlurElement::TrueBlur { commit_counter, .. } => *commit_counter,
        }
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        match self {
            BlurElement::Optimized { tex, .. } => tex.location(scale),
            BlurElement::TrueBlur { loc, .. } => *loc,
        }
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        match self {
            BlurElement::Optimized { tex, .. } => tex.src(),
            BlurElement::TrueBlur {
                src,
                transform,
                size,
                scale,
                ..
            } => src.to_buffer(*scale as f64, *transform, &size.to_f64()),
        }
    }

    fn transform(&self) -> Transform {
        match self {
            BlurElement::Optimized { tex, .. } => tex.transform(),
            BlurElement::TrueBlur { transform, .. } => *transform,
        }
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        match self {
            BlurElement::Optimized { tex, .. } => tex.damage_since(scale, commit),
            BlurElement::TrueBlur { .. } => {
                // FIXME: Damage tracking?
                DamageSet::from_slice(&[self.geometry(scale)])
            }
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        match self {
            BlurElement::Optimized { tex, .. } => tex.opaque_regions(scale),
            BlurElement::TrueBlur { .. } => {
                // Since we are rendering as true blur, we will draw whatever is behind the window
                OpaqueRegions::default()
            }
        }
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        match self {
            BlurElement::Optimized { tex, .. } => tex.geometry(scale),
            BlurElement::TrueBlur { loc, size, .. } => {
                Rectangle::new(*loc, size.to_physical_precise_round(scale))
            }
        }
    }

    fn alpha(&self) -> f32 {
        1.0
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlowRenderer> for BlurElement {
    fn draw(
        &self,
        frame: &mut GlowFrame,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        match self {
            Self::Optimized { tex, corner_radius } => {
                if *corner_radius == 0.0 {
                    <FhtTextureElement as RenderElement<GlowRenderer>>::draw(
                        &tex,
                        frame,
                        src,
                        dst,
                        damage,
                        opaque_regions,
                    )
                } else {
                    let program = Shaders::get_from_frame(frame).rounded_texture.clone();
                    let gles_frame: &mut GlesFrame = frame.borrow_mut();
                    gles_frame.override_default_tex_program(
                        program,
                        vec![
                            Uniform::new(
                                "geo",
                                [
                                    dst.loc.x as f32,
                                    dst.loc.y as f32,
                                    dst.size.w as f32,
                                    dst.size.h as f32,
                                ],
                            ),
                            Uniform::new("corner_radius", *corner_radius),
                        ],
                    );

                    let res =
                        <TextureRenderElement<GlesTexture> as RenderElement<GlesRenderer>>::draw(
                            &tex.0,
                            gles_frame,
                            src,
                            dst,
                            damage,
                            opaque_regions,
                        );

                    gles_frame.clear_tex_program_override();

                    res
                }
            }
            Self::TrueBlur { output, scale, .. } => {
                let mut fx_buffers = EffectsFramebuffers::get(output);
                let output_rect = output.geometry().to_physical(*scale);
                fx_buffers.current_buffer = CurrentBuffer::Normal;

                let shaders = Shaders::get_from_frame(frame).blur.clone();
                let frame: &mut GlesFrame = frame.borrow_mut();
                let vbos = RendererData::get_from_frame(frame).vbos;
                let supports_instancing = frame
                    .capabilities()
                    .contains(&smithay::backend::renderer::gles::Capability::Instancing);
                let debug = !frame.debug_flags().is_empty();
                let projection_matrix = glam::Mat3::from_cols_array(frame.projection());

                let gl = unsafe {
                    let mut gl = std::mem::MaybeUninit::zeroed(); // get the gl context outside of the frame to bypass borrow checker
                    let _ = frame.with_context(|gles| {
                        std::ptr::write(gl.as_mut_ptr(), gles.clone());
                    })?;
                    gl.assume_init()
                };
                let egl = frame.egl_context();

                let mut prev_fbo = 0;
                unsafe {
                    gl.GetIntegerv(ffi::FRAMEBUFFER_BINDING, &mut prev_fbo as *mut _);

                    // First get a fbo for the texture we are about to read into
                    let mut sample_fbo = 0u32;
                    {
                        gl.GenFramebuffers(1, &mut sample_fbo as *mut _);
                        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, sample_fbo);
                        gl.FramebufferTexture2D(
                            ffi::FRAMEBUFFER,
                            ffi::COLOR_ATTACHMENT0,
                            ffi::TEXTURE_2D,
                            fx_buffers.sample_buffer().tex_id(),
                            0,
                        );
                        gl.Clear(ffi::COLOR_BUFFER_BIT);
                        let status = gl.CheckFramebufferStatus(ffi::FRAMEBUFFER);
                        if status != ffi::FRAMEBUFFER_COMPLETE {
                            gl.DeleteFramebuffers(1, &mut sample_fbo as *mut _);
                            return Ok(());
                        }
                    }

                    {
                        // blit the contents
                        egl.make_current()?;

                        // NOTE: We are assured that the size of the effects texture is the same
                        // as the bound fbo size, so blitting uses dst immediatly
                        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, sample_fbo);
                        gl.BlitFramebuffer(
                            dst.loc.x,
                            dst.loc.y,
                            dst.loc.x + dst.size.w,
                            dst.loc.y + dst.size.h,
                            dst.loc.x,
                            dst.loc.y,
                            dst.loc.x + dst.size.w,
                            dst.loc.y + dst.size.h,
                            ffi::COLOR_BUFFER_BIT,
                            ffi::LINEAR,
                        );

                        if gl.GetError() == ffi::INVALID_OPERATION {
                            error!("TrueBlur needs GLES3.0 for blitting");
                            return Ok(());
                        }
                    }

                    {
                        let half_pixel = [
                            0.5 / (output_rect.size.w as f32 / 2.0),
                            0.5 / (output_rect.size.h as f32 / 2.0),
                        ];
                        for _ in 0..2 {
                            let mut render_buffer = fx_buffers.render_buffer();
                            let sample_buffer = fx_buffers.sample_buffer();
                            super::render_blur_pass_with_gl(
                                &gl,
                                &vbos,
                                debug,
                                supports_instancing,
                                projection_matrix,
                                &sample_buffer,
                                &mut render_buffer,
                                &shaders.down,
                                half_pixel,
                                &fht_compositor_config::Blur {
                                    disable: false,
                                    passes: 3,
                                    radius: 5.0,
                                },
                                src,
                                dst,
                            )?;
                            fx_buffers.current_buffer.swap();
                        }

                        let half_pixel = [
                            0.5 / (output_rect.size.w as f32 * 2.0),
                            0.5 / (output_rect.size.h as f32 * 2.0),
                        ];
                        for _ in 0..2 {
                            let mut render_buffer = fx_buffers.render_buffer();
                            let sample_buffer = fx_buffers.sample_buffer();
                            super::render_blur_pass_with_gl(
                                &gl,
                                &vbos,
                                debug,
                                supports_instancing,
                                projection_matrix,
                                &sample_buffer,
                                &mut render_buffer,
                                &shaders.up,
                                half_pixel,
                                &fht_compositor_config::Blur {
                                    disable: false,
                                    passes: 3,
                                    radius: 5.0,
                                },
                                src,
                                dst,
                            )?;
                            fx_buffers.current_buffer.swap();
                        }
                    }

                    // Cleanup
                    {
                        gl.DeleteFramebuffers(1, &mut sample_fbo as *mut _);
                        gl.BindFramebuffer(ffi::FRAMEBUFFER, prev_fbo as u32);
                    }
                }

                let last_buffer = fx_buffers.sample_buffer();
                frame.render_texture_from_to(
                    &last_buffer,
                    src,
                    dst,
                    damage,
                    opaque_regions,
                    Transform::Normal,
                    1.0,
                    None,
                    &[],
                )?;

                Ok(())
            }
        }
    }

    fn underlying_storage(&self, _: &mut GlowRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(feature = "udev-backend")]
impl<'a> RenderElement<UdevRenderer<'a>> for BlurElement {
    fn draw<'frame>(
        &self,
        frame: &mut UdevFrame<'a, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), UdevRenderError> {
        <Self as RenderElement<GlowRenderer>>::draw(
            &self,
            frame.as_mut(),
            src,
            dst,
            damage,
            opaque_regions,
        )
        .map_err(UdevRenderError::Render)
    }

    fn underlying_storage(&self, _: &mut UdevRenderer<'a>) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
