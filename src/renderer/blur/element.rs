use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::GlesError;
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::output::Output;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Transform};

use super::EffectsFramebuffers;
#[cfg(feature = "udev-backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};
use crate::renderer::texture_element::FhtTextureElement;
use crate::renderer::FhtRenderer;

/// A render element to render blurred area of the background of an [`Output`]
#[derive(Debug)]
pub struct BlurElement {
    // What we do at the end of the day is sample from the optimized_blur buffer that has been
    // prepared. We override the src argument in order to get only the region we need.
    tex: FhtTextureElement,
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
        scale: i32,
    ) -> Self {
        let fbs = &mut *EffectsFramebuffers::get(output);
        let texture = TextureRenderElement::from_static_texture(
            Id::new(),
            renderer.id(),
            loc.to_f64(),
            fbs.optimized_blur.clone(),
            scale,
            Transform::Normal,
            Some(1.0),
            Some(sample_area.to_f64()),
            Some(sample_area.size),
            // NOTE: Since this is "optimized" blur, anything below the window will not be rendered
            Some(vec![sample_area.to_buffer(
                scale,
                Transform::Normal,
                &sample_area.size,
            )]),
            Kind::Unspecified,
        );

        Self {
            tex: texture.into(),
        }
    }
}

impl Element for BlurElement {
    fn id(&self) -> &Id {
        self.tex.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.tex.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.tex.location(scale)
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.tex.src()
    }

    fn transform(&self) -> Transform {
        self.tex.transform()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.tex.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.tex.opaque_regions(scale)
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.tex.geometry(scale)
    }

    fn alpha(&self) -> f32 {
        self.tex.alpha()
    }

    fn kind(&self) -> Kind {
        self.tex.kind()
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
        <FhtTextureElement as RenderElement<GlowRenderer>>::draw(
            &self.tex,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
        )
    }

    fn underlying_storage(&self, renderer: &mut GlowRenderer) -> Option<UnderlyingStorage<'_>> {
        self.tex.underlying_storage(renderer)
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

    fn underlying_storage(&self, renderer: &mut UdevRenderer<'a>) -> Option<UnderlyingStorage<'_>> {
        self.tex.underlying_storage(renderer)
    }
}
