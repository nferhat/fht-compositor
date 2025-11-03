//! Dual-kawase blur implementation for smithay for OpenGL-based renderers ([`GlesRenderer`],
//! [`GlowRenderer`]). Implementation from the following sources
//!
//! - <https://github.com/wlrfx/scenefx>
//! - <https://github.com/alex47/Dual-Kawase-Blur>
//! - <https://www.intel.com/content/www/us/en/developer/articles/technical/an-investigation-of-fast-real-time-gpu-based-image-blur-algorithms.html>
//!
//! # How is it implemented
//!
//! The important part of this implementation is the [`BlurRegion`], which should be kept around
//! between render passes in order to do proper damage tracking. You update the region's state with
//! the received damage for the render elements that should have blur behind.
//!
//! When generating a [`BlurRegionRenderElement`], it uses the accumulated damage until the `draw()`
//! call to update the blurred contents texture (which is stored in the region) and then draws
//! that texture to the output buffer.
//!
//! # Limitations of this implementation
//!
//! The main limitation here is that if something updates **outside** the base region but in the
//! expanded blur area, the blur contents (which should sample from that region) will not get
//! updated. This is mostly a limitation of smithay's damage tracker. And to be honest, I am not
//! smart enough to make the necessary changes to make the DT account for that.
//!
//! ```
//!                        *--------------------------------*
//!                        |xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx|
//!                        |xxxxxxxx*--------------*xxxxxxxx|
//!                        |xxxxxxxx|              |xxxxxxxx|
//!                        |xxxxxxxx|    Region    |xxxxxxxx|
//!                        |xxxxxxxx|              |xxxxxxxx|
//!                        |xxxxxxxx*--------------*xxxxxxxx|
//!                        |xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx|
//!                        *--------------------------------*
//! ```
//!
//! Where xxx marks the expanded blur region
//!
//! # How to use it?
//!
//! ```no_run
//! # // Excuse the sloppy rust code, I just need a reference on how I am supposed to render this
//! fn render_elements(&self, ...) -> Vec<RenderElement> {
//!    let mut ret = vec![];
//!
//!    let blur_region = self.blur_region;
//!
//!    let initial_render_elements: Vec<_> = /* whatever */;
//!    let initial_render_elements_damage = /* depends, just get this */;
//!    ret.extend(initial_render_elements.into_iter().map(Into::into));
//!
//!    // Update the blur region state with the new damage.
//!    // On each draw call of the BlurRegionRenderElement, the damage is drained.
//!    blur_region.update(initial_render_elements_damage);
//!    let blur_region_element = blur_region.render(...);
//!    initial_render_elements.push(blur_region_element.into());
//!
//!    ret
//! }
//! ```

pub(super) mod shader;

use std::borrow::BorrowMut;
use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use anyhow::Context;
#[cfg(feature = "udev-backend")]
use glam::Mat3;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{AsRenderElements, Element, Id, Kind, RenderElement};
use smithay::backend::renderer::gles::format::fourcc_to_gl_formats;
use smithay::backend::renderer::gles::{ffi, Capability, GlesError, GlesRenderer, GlesTexture};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::{
    CommitCounter, DamageBag, DamageSet, DamageSnapshot, OpaqueRegions,
};
use smithay::backend::renderer::{
    Bind, Blit, ContextId, Frame, Offscreen, Renderer, Texture, TextureFilter,
};
use smithay::desktop::layer_map_for_output;
use smithay::output::Output;
use smithay::reexports::gbm::Format;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::shell::wlr_layer::Layer;

use super::data::RendererData;
use super::shaders::Shaders;
use super::{render_elements, FhtRenderer};
#[cfg(feature = "udev-backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};
use crate::output::OutputExt;

/// [`BlurRegion`] parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parameters {
    /// The border radius.
    pub corner_radius: f32,
    /// The number of blur passes.
    pub passes: usize,
    /// The blur radius of each pass.
    pub radius: f32,
}

/// A blur region.
///
/// You should store this region and keep it updated with damage.
#[derive(Debug)]
pub struct BlurRegion {
    id: Id,
    /// Associated data for this region. Created whenever
    inner: RefCell<Option<BlurRegionInner>>,
    /// Parameters/configuration for this blur region
    // NOTE: Animating these parameters would be REALLY expensive and pretty pointless.
    parameters: Parameters,
    // Since we need todo some handling when destroying the inner blur region, we should only reset
    // when doing [`BlurRegion::render`], notably to detroy the framebuffer object we create.
    should_reset: bool,
}

#[derive(Debug)]
struct BlurRegionInner {
    /// The texture backing this blur region.
    ///
    /// We store the contents of the buffer behind for two reasons:
    /// 1. Avoid excessive blits when updating the buffer
    /// 2. In case there was no damage between two draw passes, we can just re-use the buffer as
    ///    it, shaving off a bit of (somewhat expensive) blur operations
    ///
    /// We also cache the framebuffer object created for this texture
    buffer: (GlesTexture, u32),
    /// The [`ContextId`] for the texture's renderer.
    context_id: ContextId<GlesTexture>,
    /// The accumulated damage between each draw call. When calling [`BlurRegion::render`], its
    /// this damage bag that is passed onto the render element.
    ///
    /// The damage is relative to the backing texture's origin.
    damage: DamageBag<i32, Buffer>,
}

impl BlurRegion {
    /// Create a new [`BlurRegion`] with given [`Parameters`]
    pub fn new(parameters: Parameters) -> Self {
        Self {
            id: Id::new(),
            parameters,
            inner: RefCell::default(),
            // Doesn't really matter cause the inner isn't created yet.
            should_reset: true,
        }
    }

    /// Update the blur parameters of this region.
    ///
    /// Doing this will damage the full region, and recreates the buffer.
    pub fn update_parameters(&mut self, parameters: Parameters) {
        let changed = self.parameters != parameters;
        if changed {
            self.parameters = parameters;
            // FIXME: We should only damage the corners if corner radius changed only.
            self.should_reset = true;
        }
    }

    /// Resize the blur region.
    ///
    /// Doing this will damage the full region, and recreates the buffer.
    pub fn resize(&mut self, new_size: Size<i32, Logical>) {
        let inner = self.inner.borrow_mut();
        self.should_reset = inner.as_ref().map_or(true, |inner| {
            let src_size = new_size.to_buffer(1, Transform::Normal);
            inner.buffer.0.size() != src_size
        });
    }

    /// Push a [`DamageSnapshot`] of damage to the [`BlurRegion`]
    pub fn push_damage(&self, damage: impl IntoIterator<Item = Rectangle<i32, Buffer>>) {
        let mut inner = self.inner.borrow_mut();
        // On initial render we do a full damage pass.
        let Some(inner) = inner.as_mut() else { return };

        let expand_size =
            (2f32.powi(self.parameters.passes as i32 + 1) * self.parameters.radius).ceil() as i32;
        // Each damage rectangles sample from a region all around it. The amount is calculated
        // above. Now just expand in every direction.
        inner.damage.add(damage.into_iter().map(|mut rect| {
            rect.size += (2 * expand_size, 2 * expand_size).into();
            rect.loc -= (expand_size, expand_size).into();
            rect
        }));
    }

    pub fn render(
        &self,
        renderer: &mut impl FhtRenderer,
        region: Rectangle<i32, Logical>,
        alpha: f32,
    ) -> anyhow::Result<BlurRegionRenderElement> {
        crate::profile_function!();
        let renderer = renderer.glow_renderer_mut();
        let context_id = renderer.context_id();

        let src_size = region.size;
        let src_size = src_size.to_buffer(1, Transform::Normal);

        let mut inner = self.inner.borrow_mut();
        // We first must ensure that we created the inner state.
        let mut reset = false;
        if let Some(BlurRegionInner {
            context_id: buffer_ctx_id,
            buffer,
            ..
        }) = &mut *inner
        {
            // Reset if we are rendering in another renderer (IE another output) or if the size
            // changed. For now, this causes full damage.
            if *buffer_ctx_id != context_id {
                trace!("Resetting blur buffer due to different context ID");
                reset = true;
            }

            let buffer_size = buffer.0.size();
            if buffer_size != src_size {
                trace!("Resetting blur buffer due to size mismatch");
                reset = true;
            }
        }
        if self.should_reset {
            trace!("Resetting blur buffer due to reset flag");
        }

        if reset {
            if let Some(BlurRegionInner {
                buffer: (_, mut fbo),
                ..
            }) = inner.take()
            {
                // Dont' forget to delete the previous FBO
                let gles_renderer: &mut GlesRenderer = renderer.borrow_mut();
                gles_renderer
                    .with_context(|gl| unsafe { gl.DeleteFramebuffers(1, &mut fbo as *mut _) })?;
            }
        }

        let inner = match &mut *inner {
            Some(inner) => inner,
            // The blur buffer should never be transparent,
            None => {
                let buffer = renderer.create_buffer(Format::Xrgb8888, src_size)?;
                let fbo = create_fbo_for_texture(renderer.borrow_mut(), &buffer)?;

                inner.get_or_insert(BlurRegionInner {
                    buffer: (buffer, fbo),
                    context_id: renderer.context_id(),
                    damage: DamageBag::default(),
                })
            }
        };

        Ok(BlurRegionRenderElement {
            context_id,
            id: self.id.clone(),
            buffer: inner.buffer.clone(),
            // FIXME: Proper scaling
            scale: Scale::from(1.0),
            damage: inner.damage.snapshot(),
            location: region.loc,
            src: src_size,
            alpha,
        })
    }
}

#[derive(Debug)]
pub struct BlurRegionRenderElement {
    id: Id,
    buffer: (GlesTexture, u32),
    context_id: ContextId<GlesTexture>,
    scale: Scale<f64>,
    damage: DamageSnapshot<i32, Buffer>,
    location: Point<i32, Logical>,
    src: Size<i32, Buffer>,
    alpha: f32,
}

impl BlurRegionRenderElement {
    pub fn logical_size(&self) -> Size<i32, Logical> {
        self.src
            .to_f64()
            .to_logical(self.scale, Transform::Normal)
            .to_i32_round()
    }

    fn damage_since(&self, commit: Option<CommitCounter>) -> DamageSet<i32, Buffer> {
        self.damage
            .damage_since(commit)
            .unwrap_or_else(|| DamageSet::from_slice(&[Rectangle::from_size(self.buffer.0.size())]))
    }
}

impl Element for BlurRegionRenderElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.damage.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size(self.src).to_f64()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        let logical_geo = Rectangle::new(self.location, self.logical_size());
        logical_geo.to_physical_precise_round(scale)
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.location.to_physical_precise_round(scale)
    }

    fn transform(&self) -> Transform {
        Transform::Normal
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        let texture_size = self.buffer.0.size().to_f64();
        let src = self.src();

        self.damage_since(commit)
            .into_iter()
            .filter_map(|region| {
                let mut region = region.to_f64().intersection(src)?;

                region.loc -= src.loc;
                region = region.upscale(texture_size / src.size);

                let logical = region.to_logical(self.scale, Transform::Normal, &src.size);
                Some(logical.to_physical_precise_up(scale))
            })
            .collect()
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        // No opaque regions here.
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        // It's obvious why we should not use Cursor element kind.
        // However, for ScanoutCandidate, the blur buffer is always drawn behind something (a
        // window, layer-shell, or whatever)
        Kind::Unspecified
    }
}

impl RenderElement<GlowRenderer> for BlurRegionRenderElement {
    fn draw(
        &self,
        frame: &mut GlowFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        if frame.context_id() != self.context_id {
            warn!("trying to render texture from different renderer");
            return Ok(());
        }

        Ok(())
    }
}

#[cfg(feature = "udev-backend")]
impl<'a> RenderElement<UdevRenderer<'a>> for BlurRegionRenderElement {
    fn draw(
        &self,
        frame: &mut UdevFrame<'a, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), UdevRenderError> {
        let gles_frame = frame.as_mut();
        RenderElement::<GlowRenderer>::draw(&self, gles_frame, src, dst, damage, opaque_regions)?;
        Ok(())
    }
}

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
        let scale = output.current_scale().integer_scale();
        let output_size = output.geometry().size.to_physical(scale);

        fn create_buffer(
            renderer: &mut GlowRenderer,
            size: Size<i32, Physical>,
        ) -> Result<GlesTexture, GlesError> {
            renderer.create_buffer(
                Format::Abgr8888,
                size.to_logical(1).to_buffer(1, Transform::Normal),
            )
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
        let scale = output.current_scale().integer_scale();
        let output_size = output.geometry().size.to_physical(scale);

        fn create_buffer(
            renderer: &mut GlowRenderer,
            size: Size<i32, Physical>,
        ) -> Result<GlesTexture, GlesError> {
            renderer.create_buffer(
                Format::Abgr8888,
                size.to_logical(1).to_buffer(1, Transform::Normal),
            )
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

        renderer
            .blit(
                &tex_fb,
                &mut optimized_blur_fb,
                Rectangle::from_size(output_rect.size),
                Rectangle::from_size(output_rect.size),
                TextureFilter::Linear,
            )?
            .wait()?;

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
    blur_program: &shader::BlurShader,
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
        gl.TexParameteri(
            ffi::TEXTURE_2D,
            ffi::TEXTURE_WRAP_S,
            ffi::MIRRORED_REPEAT as i32,
        );
        gl.TexParameteri(
            ffi::TEXTURE_2D,
            ffi::TEXTURE_WRAP_T,
            ffi::MIRRORED_REPEAT as i32,
        );

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

/// Create a framebuffer object for a texture.
fn create_fbo_for_texture(
    renderer: &mut GlesRenderer,
    texture: &GlesTexture,
) -> Result<u32, GlesError> {
    unsafe {
        renderer.egl_context().make_current()?;
    }

    // FIXME: In smithay GlesRenderer::bind_texture, we wait for the texture sync before doing
    // the operations, but here, we don't have access to the private sync_points.
    let mut fbo = 0;
    let tex_id = texture.tex_id();
    renderer.with_context(|gl| unsafe {
        gl.GenFramebuffers(1, &mut fbo);
        gl.BindFramebuffer(ffi::FRAMEBUFFER, fbo);
        gl.FramebufferTexture2D(
            ffi::FRAMEBUFFER,
            ffi::COLOR_ATTACHMENT0,
            ffi::TEXTURE_2D,
            tex_id,
            0,
        );

        // Check for error status. Also reset since we don't really need the framebuffer immediatly
        let status = gl.CheckFramebufferStatus(ffi::FRAMEBUFFER);
        gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);

        if status != ffi::FRAMEBUFFER_COMPLETE {
            gl.DeleteFramebuffers(1, &mut fbo);
            return Err(GlesError::FramebufferBindingError);
        }

        Ok(fbo)
    })?
}

/*
// Renders a blur pass using gl code bypassing smithay's Frame mechanisms
//
// When rendering blur in real-time (for windows, for example) there should not be a wait for
// fencing/finishing since this will be done when sending the fb to the output. Using a Frame
// forces us to do that.
#[allow(clippy::too_many_arguments)]
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
    crate::profile_function!();
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
        crate::profile_scope!("bind to render texture");
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

    {
        crate::profile_scope!("render blur pass to texture");

        let mat = projection_matrix;
        // NOTE: We are assured that tex_size != 0, and src.size != too (by damage tracker)
        let mut tex_mat = super::build_texture_mat(src, dest, tex_size, Transform::Normal);
        if sample_buffer.is_y_inverted() {
            tex_mat *= Mat3::from_cols_array(&[1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0]);
        }

        gl.Disable(ffi::BLEND);

        // FIXME: Use actual damage for this? Would require making a custom window render element
        // that includes blur and whatnot to get the damage for the window only
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
    }

    // Clean up
    {
        crate::profile_scope!("clean up blur pass");
        gl.Enable(ffi::BLEND);
        gl.DeleteFramebuffers(1, &render_buffer_fbo as *const _);
        gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
        gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn get_main_buffer_blur(
    gl: &ffi::Gles2,
    fx_buffers: &mut EffectsFramebuffers,
    shaders: &BlurShaders,
    blur_config: &fht_compositor_config::Blur,
    projection_matrix: Mat3,
    scale: i32,
    vbos: &[u32; 2],
    debug: bool,
    supports_instancing: bool,
    // dst is the region that we want blur on
    dst: Rectangle<i32, Physical>,
) -> Result<GlesTexture, GlesError> {
    let tex_size = fx_buffers
        .effects
        .size()
        .to_logical(1, Transform::Normal)
        .to_physical(scale);
    let dst_expanded = {
        let mut dst = dst;
        let size = (2f32.powi(blur_config.passes as i32 + 1) * blur_config.radius).ceil() as i32;
        dst.loc -= Point::from((size, size));
        dst.size += Size::from((size, size)).upscale(2);
        dst
    };

    let mut prev_fbo = 0;
    gl.GetIntegerv(ffi::FRAMEBUFFER_BINDING, &mut prev_fbo as *mut _);

    let (sample_buffer, _) = fx_buffers.buffers();

    // First get a fbo for the texture we are about to read into
    let mut sample_fbo = 0u32;
    {
        gl.GenFramebuffers(1, &mut sample_fbo as *mut _);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, sample_fbo);
        gl.FramebufferTexture2D(
            ffi::FRAMEBUFFER,
            ffi::COLOR_ATTACHMENT0,
            ffi::TEXTURE_2D,
            sample_buffer.tex_id(),
            0,
        );
        gl.Clear(ffi::COLOR_BUFFER_BIT);
        let status = gl.CheckFramebufferStatus(ffi::FRAMEBUFFER);
        if status != ffi::FRAMEBUFFER_COMPLETE {
            gl.DeleteFramebuffers(1, &mut sample_fbo as *mut _);
            return Err(GlesError::FramebufferBindingError);
        }
    }

    {
        // NOTE: We are assured that the size of the effects texture is the same
        // as the bound fbo size, so blitting uses dst immediatly
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, sample_fbo);
        gl.BlitFramebuffer(
            dst_expanded.loc.x,
            dst_expanded.loc.y,
            dst_expanded.loc.x + dst_expanded.size.w,
            dst_expanded.loc.y + dst_expanded.size.h,
            dst_expanded.loc.x,
            dst_expanded.loc.y,
            dst_expanded.loc.x + dst_expanded.size.w,
            dst_expanded.loc.y + dst_expanded.size.h,
            ffi::COLOR_BUFFER_BIT,
            ffi::LINEAR,
        );

        if gl.GetError() == ffi::INVALID_OPERATION {
            error!("TrueBlur needs GLES3.0 for blitting");
            return Err(GlesError::BlitError);
        }
    }

    {
        let passes = blur_config.passes;
        let half_pixel = [
            0.5 / (tex_size.w as f32 / 2.0),
            0.5 / (tex_size.h as f32 / 2.0),
        ];
        for i in 0..passes {
            let (sample_buffer, render_buffer) = fx_buffers.buffers();
            let damage = dst_expanded.downscale(1 << (i + 1));
            render_blur_pass_with_gl(
                gl,
                vbos,
                debug,
                supports_instancing,
                projection_matrix,
                sample_buffer,
                render_buffer,
                scale,
                &shaders.down,
                half_pixel,
                blur_config,
                damage,
            )?;
            fx_buffers.current_buffer.swap();
        }

        let half_pixel = [
            0.5 / (tex_size.w as f32 * 2.0),
            0.5 / (tex_size.h as f32 * 2.0),
        ];
        for i in 0..passes {
            let (sample_buffer, render_buffer) = fx_buffers.buffers();
            let damage = dst_expanded.downscale(1 << (passes - 1 - i));
            render_blur_pass_with_gl(
                gl,
                vbos,
                debug,
                supports_instancing,
                projection_matrix,
                sample_buffer,
                render_buffer,
                scale,
                &shaders.up,
                half_pixel,
                blur_config,
                damage,
            )?;
            fx_buffers.current_buffer.swap();
        }
    }

    // Cleanup
    {
        gl.DeleteFramebuffers(1, &mut sample_fbo as *mut _);
        gl.BindFramebuffer(ffi::FRAMEBUFFER, prev_fbo as u32);
    }

    Ok(fx_buffers.effects.clone())
}
*/
