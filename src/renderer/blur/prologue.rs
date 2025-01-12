use std::borrow::BorrowMut;

use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};
use smithay::backend::renderer::gles::{ffi, GlesFrame, GlesTarget};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{Bind, Blit, Texture, TextureFilter, Unbind};
use smithay::output::Output;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Transform};

use super::EffectsFramebuffers;
#[cfg(feature = "udev-backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};
use crate::renderer::AsGlowFrame;

/// A render element to prepare the blur buffer.
///
/// This element does actually **nothing**, it just renders into the saved blur buffer associated
/// with the output. You must add this element to your render element list BEFORE using any
/// `BlurRenderElement`
///
/// ## Usage
///
/// You put this somewhere in the vector you generate render elements in, anything added after this
/// element will be blurred and rendered into a texture buffer
#[derive(Debug)]
pub struct BlurPrologueElement {
    id: Id,
    /// The output content we are currently blurring
    output: Output,
    commit_counter: CommitCounter,
}

impl BlurPrologueElement {
    pub fn new(id: Id, output: Output) -> Self {
        Self {
            id,
            output,
            commit_counter: CommitCounter::default(),
        }
    }
}

impl Element for BlurPrologueElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit_counter
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        // NOTE: i can't give this a zero size otherwise the damage tracker will just skip it
        Rectangle::new((0., 0.).into(), (1., 1.).into())
    }

    fn geometry(&self, _: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::new((0, 0).into(), (1, 1).into())
    }

    fn location(&self, _: Scale<f64>) -> Point<i32, Physical> {
        Point::from((0, 0))
    }

    fn transform(&self) -> Transform {
        Transform::Normal
    }

    fn damage_since(&self, _: Scale<f64>, _: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        DamageSet::from_slice(&[Rectangle::new((0, 0).into(), (1, 1).into())])
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        1.0
    }

    fn kind(&self) -> Kind {
        Kind::default()
    }
}

impl RenderElement<GlowRenderer> for BlurPrologueElement {
    fn draw(
        &self,
        frame: &mut GlowFrame,
        _: Rectangle<f64, Buffer>,
        _: Rectangle<i32, Physical>,
        _: &[Rectangle<i32, Physical>],
        _: &[Rectangle<i32, Physical>],
    ) -> Result<(), <GlowRenderer as smithay::backend::renderer::Renderer>::Error> {
        let gles_frame: &mut GlesFrame = frame.borrow_mut();

        gles_frame.with_context(|gl| unsafe {
            // Seems like swayfx disables these, I'll comply
            gl.Disable(ffi::BLEND);
            gl.Disable(ffi::STENCIL_TEST);
        })?;

        gles_frame.with_renderer(|renderer| {
            // First render from the current bound buffer (containing whatever we rendered so far)
            // into temporary render buffers. get_main_buffer_blur takes care of rebinding after
            // us
            let buffer = super::get_main_buffer_blur(renderer, &self.output);
            let size = buffer
                .size()
                .to_logical(1, Transform::Normal)
                .to_physical(1);

            // Now blit from render buffer into optimized_blur_saved buffer
            // The optimized_blur buffer might not always be dirty, so we save it.
            let buffers = &mut *EffectsFramebuffers::get(&self.output);
            let previous_target = renderer.target_mut().take();
            renderer.bind(buffer).expect("gl should bind");
            renderer
                .blit_to(
                    buffers.optimized_blur.clone(),
                    Rectangle::from_size(size),
                    Rectangle::from_size(size),
                    TextureFilter::Linear,
                )
                .expect("Failed to blit to optimized_blur");
            renderer.unbind().unwrap();

            match previous_target {
                Some(ref target) => match target {
                    // NOTE: The drop impl of GlesTarget will automatically send destruction events
                    GlesTarget::Image { dmabuf, .. } => renderer.bind(dmabuf.clone()).unwrap(),
                    GlesTarget::Surface { surface } => renderer.bind(surface.clone()).unwrap(),
                    GlesTarget::Texture { texture, .. } => renderer.bind(texture.clone()).unwrap(),
                    GlesTarget::Renderbuffer { buf, .. } => renderer.bind(buf.clone()).unwrap(),
                },
                None => (), // we werent even bound yet!
            }
        })?;

        gles_frame.with_context(|gl| unsafe {
            // Seems like swayfx disables these, I'll comply
            gl.Enable(ffi::BLEND);
            gl.Enable(ffi::STENCIL_TEST);
        })?;

        Ok(())
    }
}

#[cfg(feature = "udev-backend")]
impl<'a> RenderElement<UdevRenderer<'a>> for BlurPrologueElement {
    fn draw(
        &self,
        frame: &mut UdevFrame<'a, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        or: &[Rectangle<i32, Physical>],
    ) -> Result<(), UdevRenderError> {
        let glow_frame: &mut GlowFrame = frame.glow_frame_mut();
        <Self as RenderElement<GlowRenderer>>::draw(&self, glow_frame, src, dst, damage, or)
            .map_err(UdevRenderError::Render)
    }
}
