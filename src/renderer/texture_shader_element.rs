use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};
use smithay::backend::renderer::gles::element::TextureShaderElement;
use smithay::backend::renderer::gles::GlesError;
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Transform};

#[cfg(feature = "udev-backend")]
use super::AsGlowFrame;
#[cfg(feature = "udev-backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};

/// NewType wrapper to impl [`TextureShaderElement`] for [`UdevRenderer`]
#[derive(Debug)]
pub struct FhtTextureShaderElement(pub TextureShaderElement);

impl From<TextureShaderElement> for FhtTextureShaderElement {
    fn from(value: TextureShaderElement) -> Self {
        Self(value)
    }
}

impl Element for FhtTextureShaderElement {
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

    fn alpha(&self) -> f32 {
        self.0.alpha()
    }

    fn kind(&self) -> Kind {
        self.0.kind()
    }
}

impl RenderElement<GlowRenderer> for FhtTextureShaderElement {
    fn draw(
        &self,
        frame: &mut GlowFrame<'_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        <TextureShaderElement as RenderElement<GlowRenderer>>::draw(
            &self.0,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
        )
    }

    fn underlying_storage(
        &self,
        renderer: &mut GlowRenderer,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage> {
        self.0.underlying_storage(renderer)
    }
}

#[cfg(feature = "udev-backend")]
impl<'a> RenderElement<UdevRenderer<'a>> for FhtTextureShaderElement {
    fn draw(
        &self,
        frame: &mut UdevFrame<'a, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), UdevRenderError> {
        let frame = frame.glow_frame_mut();
        <TextureShaderElement as RenderElement<GlowRenderer>>::draw(
            &self.0,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
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
