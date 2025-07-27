use std::borrow::BorrowMut;

use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, GlesRenderer, GlesTexture, Uniform};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::Renderer as _;
use smithay::output::Output;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use super::{CurrentBuffer, EffectsFramebuffers};
#[cfg(feature = "udev-backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};
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
        noise: f32,
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
        src: Rectangle<f64, Logical>,
        size: Size<i32, Logical>,
        corner_radius: f32,
        loc: Point<i32, Physical>,
        output: Output,
        alpha: f32,
        // FIXME: Use DamageBag and expand it as needed?
        commit_counter: CommitCounter,
        blur_config: fht_compositor_config::Blur,
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        renderer: &mut impl FhtRenderer,
        output: &Output,
        sample_area: Rectangle<i32, Logical>,
        loc: Point<i32, Physical>,
        corner_radius: f32,
        optimized: bool,
        scale: i32,
        alpha: f32,
        blur_config: fht_compositor_config::Blur,
    ) -> Self {
        let fbs = &mut *EffectsFramebuffers::get(output);
        let texture = fbs.optimized_blur.clone();

        if optimized {
            let renderer: &mut GlesRenderer = renderer.glow_renderer_mut().borrow_mut();
            let texture = TextureRenderElement::from_static_texture(
                Id::new(),
                renderer.context_id(),
                loc.to_f64(),
                texture,
                scale,
                Transform::Normal,
                Some(alpha),
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
                noise: blur_config.noise,
            }
        } else {
            Self::TrueBlur {
                id: Id::new(),
                scale,
                src: sample_area.to_f64(),
                transform: Transform::Normal,
                size: sample_area.size,
                corner_radius,
                loc,
                alpha,
                output: output.clone(), // fixme i hate this
                commit_counter: CommitCounter::default(),
                blur_config,
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
            BlurElement::TrueBlur { blur_config, .. } => {
                // Since the blur element samples from around itself, we must expand the damage it
                // induces to include any potential changes.
                let mut geometry = Rectangle::from_size(self.geometry(scale).size);
                let size =
                    (2f32.powi(blur_config.passes as i32 + 1) * blur_config.radius).ceil() as i32;
                geometry.loc -= Point::from((size, size));
                geometry.size += Size::from((size, size)).upscale(2);

                // FIXME: Damage tracking?
                DamageSet::from_slice(&[geometry])
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
            Self::Optimized {
                tex,
                corner_radius,
                noise,
            } => {
                if *corner_radius == 0.0 {
                    <FhtTextureElement as RenderElement<GlowRenderer>>::draw(
                        tex,
                        frame,
                        src,
                        dst,
                        damage,
                        opaque_regions,
                    )
                } else {
                    let program = Shaders::get_from_frame(frame.borrow_mut())
                        .blur_finish
                        .clone();
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
                            Uniform::new("noise", *noise),
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
            Self::TrueBlur {
                output,
                scale,
                corner_radius,
                blur_config,
                alpha,
                ..
            } => {
                let mut fx_buffers = EffectsFramebuffers::get(output);
                fx_buffers.current_buffer = CurrentBuffer::Normal;

                let gles_frame: &mut GlesFrame = frame.borrow_mut();
                let shaders = Shaders::get_from_frame(gles_frame).blur.clone();
                let vbos = RendererData::get_from_frame(gles_frame).vbos;
                let supports_instancing = gles_frame
                    .capabilities()
                    .contains(&smithay::backend::renderer::gles::Capability::Instancing);
                let debug = !gles_frame.debug_flags().is_empty();
                let projection_matrix = glam::Mat3::from_cols_array(gles_frame.projection());

                // Update the blur buffers.
                // We use gl ffi directly to circumvent some stuff done by smithay
                let blurred_texture = gles_frame.with_context(|gl| unsafe {
                    super::get_main_buffer_blur(
                        gl,
                        &mut fx_buffers,
                        &shaders,
                        blur_config,
                        projection_matrix,
                        *scale,
                        &vbos,
                        debug,
                        supports_instancing,
                        dst,
                    )
                })??;

                let (program, additional_uniforms) = if *corner_radius == 0.0 {
                    (None, vec![])
                } else {
                    let program = Shaders::get_from_frame(gles_frame).blur_finish.clone();
                    (
                        Some(program),
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
                            Uniform::new("noise", blur_config.noise),
                        ],
                    )
                };

                gles_frame.render_texture_from_to(
                    &blurred_texture,
                    src,
                    dst,
                    damage,
                    opaque_regions,
                    Transform::Normal,
                    *alpha,
                    program.as_ref(),
                    &additional_uniforms,
                )
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
            self,
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
