use std::time::Duration;

use fht_animation::Animation;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::utils::{
    Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::{Element as _, Id, Kind};
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::Color32F;
use smithay::utils::{Logical, Point, Rectangle, Scale, Transform};

use super::tile::TileRenderElement;
use crate::fht_render_elements;
use crate::renderer::render_to_texture;
use crate::renderer::texture_element::FhtTextureElement;

const CLOSE_SCALE_THRESHOLD: f64 = 0.8;
const FALLBACK_BUFFER_COLOR: Color32F = Color32F::new(1.0, 0.0, 0.0, 1.0);

/// A representation of a closing [`Tile`](super::tile::Tile).
///
/// When the tile's [`Window`](crate::window::Window) gets unmapped, or dies, the tile will have
/// prepared in advance a list of render elements that constitutes the "snapshot" or the last frame
/// before unmapped, used to animate a closing effect where the tile pops out.
#[derive(Debug)]
pub struct ClosingTile {
    /// The texture with the tile's render elements.
    ///
    /// If this is [`None`], a [`SolidColorBuffer`] will get rendered as a fallback.
    texture: Option<(TextureBuffer<GlesTexture>, Point<i32, Logical>)>,
    /// The last registered tile geometry.
    geometry: Rectangle<i32, Logical>,
    /// The animation.
    progress: Animation<f64>,
}

fht_render_elements! {
    ClosingTileRenderElement => {
        Texture = RelocateRenderElement<RescaleRenderElement<FhtTextureElement>>,
        Solid = SolidColorRenderElement,
    }
}

impl ClosingTile {
    pub fn new(
        renderer: &mut GlowRenderer,
        render_elements: Vec<TileRenderElement<GlowRenderer>>,
        geometry: Rectangle<i32, Logical>,
        scale: Scale<f64>,
        animation: &super::AnimationConfig,
    ) -> Self {
        let geo = render_elements
            .iter()
            .fold(Rectangle::default(), |acc, e| acc.merge(e.geometry(scale)));
        let render_elements = render_elements.into_iter().rev().map(|e| {
            RelocateRenderElement::from_element(e, (-geo.loc.x, -geo.loc.y), Relocate::Relative)
        });

        let texture = match render_to_texture(
            renderer,
            geo.size,
            scale,
            Transform::Normal,
            Fourcc::Abgr8888,
            render_elements.into_iter(),
        ) {
            Ok((texture, _)) => {
                let texture = TextureBuffer::from_texture(
                    renderer,
                    texture,
                    scale.x.max(scale.y) as i32,
                    Transform::Normal,
                    None,
                );
                Some((texture, geo.loc.to_f64().to_logical(scale).to_i32_round()))
            }
            Err(err) => {
                warn!(?err, "Failed to render texture for ClosingTile");
                None
            }
        };

        Self {
            texture,
            geometry,
            progress: Animation::new(1.0, 0.0, animation.duration).with_curve(animation.curve),
        }
    }

    pub fn advance_animations(&mut self, target_presentation_time: Duration) {
        self.progress.tick(target_presentation_time);
    }

    /// Did we finish animating the closing animation.
    pub fn is_finished(&self) -> bool {
        self.progress.is_finished()
    }

    /// Render this [`ClosingTile`].
    ///
    /// NOTE: It is up to YOU to assure that the rendered that will draw the
    /// [`ClosingTileRenderElement`] is the same one used to create the [`ClosingTile`].
    pub fn render(&self, scale: i32, alpha: f32) -> ClosingTileRenderElement {
        let Some((texture, offset)) = &self.texture else {
            return SolidColorRenderElement::new(
                Id::new(),
                self.geometry.to_physical_precise_round(scale),
                CommitCounter::default(),
                FALLBACK_BUFFER_COLOR,
                Kind::Unspecified,
            )
            .into();
        };
        let progress = self.progress.value();

        let texture: FhtTextureElement = TextureRenderElement::from_texture_buffer(
            Point::from((0., 0.)),
            texture,
            Some(progress.clamp(0., 1.) as f32 * alpha),
            None,
            None,
            Kind::Unspecified,
        )
        .into();

        let center = self.geometry.size.to_point().downscale(2);
        let origin = (center + *offset).to_physical_precise_round(scale);
        let rescale = progress * (1.0 - CLOSE_SCALE_THRESHOLD) + CLOSE_SCALE_THRESHOLD;
        let rescale = RescaleRenderElement::from_element(texture, origin, rescale);

        let location = (self.geometry.loc + *offset).to_physical_precise_round(scale);
        let relocate = RelocateRenderElement::from_element(rescale, location, Relocate::Relative);
        ClosingTileRenderElement::Texture(relocate)
    }
}
