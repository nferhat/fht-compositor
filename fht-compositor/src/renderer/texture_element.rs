use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};
use smithay::backend::renderer::gles::{GlesError, GlesTexture};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::multigpu::MultiTexture;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::Texture;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Transform};

#[cfg(feature = "udev_backend")]
use super::AsGlowFrame;
#[cfg(feature = "udev_backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};

/// A newtype struct around TextureRenderElement<GlesTexture> for it to implement
/// RenderElement<UdevRenderer>
#[derive(Debug)]
pub struct FhtTextureElement<E = GlesTexture>(pub TextureRenderElement<E>)
where
    E: Texture + Clone + 'static;

impl<E: Texture + Clone + 'static> Element for FhtTextureElement<E> {
    fn id(&self) -> &Id {
        self.0.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.0.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.0.src()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.0.geometry(scale)
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.geometry(scale).loc
    }

    fn transform(&self) -> Transform {
        Transform::Normal
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> smithay::backend::renderer::utils::DamageSet<i32, Physical> {
        self.0.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        self.0.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.0.alpha()
    }

    fn kind(&self) -> Kind {
        self.0.kind()
    }
}

impl RenderElement<GlowRenderer> for FhtTextureElement<GlesTexture> {
    fn draw(
        &self,
        frame: &mut GlowFrame<'_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        <TextureRenderElement<GlesTexture> as RenderElement<GlowRenderer>>::draw(
            &self.0, frame, src, dst, damage,
        )
    }

    fn underlying_storage(
        &self,
        renderer: &mut GlowRenderer,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage> {
        self.0.underlying_storage(renderer)
    }
}

#[cfg(feature = "udev_backend")]
impl<'a> RenderElement<UdevRenderer<'a>> for FhtTextureElement<MultiTexture> {
    fn draw(
        &self,
        frame: &mut UdevFrame<'a, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), UdevRenderError<'a>> {
        <TextureRenderElement<MultiTexture> as RenderElement<UdevRenderer<'a>>>::draw(
            &self.0, frame, src, dst, damage,
        )
    }

    fn underlying_storage(
        &self,
        _: &mut UdevRenderer<'a>,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage> {
        None // pixel shader elements can't be scanned out.
    }
}

#[cfg(feature = "udev_backend")]
impl<'a> RenderElement<UdevRenderer<'a>> for FhtTextureElement<GlesTexture> {
    fn draw(
        &self,
        frame: &mut UdevFrame<'a, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), UdevRenderError<'a>> {
        let frame = frame.glow_frame_mut();
        <TextureRenderElement<GlesTexture> as RenderElement<GlowRenderer>>::draw(
            &self.0, frame, src, dst, damage,
        )
        .map_err(UdevRenderError::Render)
    }

    fn underlying_storage(
        &self,
        _: &mut UdevRenderer<'a>,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage> {
        None // pixel shader elements can't be scanned out.
    }
}
