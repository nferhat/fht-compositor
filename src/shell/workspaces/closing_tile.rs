use std::cell::{OnceCell, RefCell};
use std::time::Duration;

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
use smithay::utils::{Logical, Point, Rectangle, Scale, Size, Transform};
use fht_animation::{Animation, AnimationCurve};

use super::tile::TileRenderElement;
use crate::fht_render_elements;
use crate::renderer::texture_element::FhtTextureElement;
use crate::renderer::{render_to_texture, FhtRenderer};

const CLOSE_SCALE_THRESHOLD: f64 = 0.8;

pub struct ClosingTile {
    // we render inside the texture buffer lazily on first render
    // Option in the case that we fail to render into the texture
    render_elements: RefCell<Vec<TileRenderElement<GlowRenderer>>>,
    texture: OnceCell<Option<(TextureBuffer<GlesTexture>, Point<i32, Logical>)>>,
    size: Size<i32, Logical>,
    // We render the closing tile above all other tiles, the user can't interact with it, and its
    // rendered in place before removing itself
    //
    // We don't render with a border since we don't really know if the tile was focused or not,
    // plus, its not a really noticable detail
    location: Point<i32, Logical>,
    progress: Animation<f64>,
}

impl ClosingTile {
    pub fn new(
        render_elements: Vec<TileRenderElement<GlowRenderer>>,
        location: Point<i32, Logical>,
        size: Size<i32, Logical>,
        duration: Duration,
        curve: AnimationCurve,
    ) -> Self {
        Self {
            render_elements: RefCell::new(render_elements),
            texture: OnceCell::new(),
            size,
            location,
            progress: Animation::new(1.0, 0.0, duration).with_curve(curve),
        }
    }

    pub fn advance_animations(&mut self, now: Duration) {
        self.progress.tick(now);
    }

    pub fn is_finished(&self) -> bool {
        self.progress.is_finished()
    }

    pub fn render<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
        alpha: f32,
    ) -> ClosingTileRenderElement {
        let Some((texture, offset)) = self.texture.get_or_init(|| {
            let render_elements = std::mem::take(&mut *self.render_elements.borrow_mut());
            let geo = render_elements
                .iter()
                .fold(Rectangle::default(), |acc, e| acc.merge(e.geometry(scale)));
            let render_elements = render_elements.into_iter().rev().map(|e| {
                RelocateRenderElement::from_element(e, (-geo.loc.x, -geo.loc.y), Relocate::Relative)
            });

            let Ok(texture) = render_to_texture(
                renderer.glow_renderer_mut(),
                geo.size,
                scale,
                Transform::Normal,
                Fourcc::Abgr8888,
                render_elements.into_iter(),
            )
            .map(|(tex, _)| tex)
            .map_err(|err| warn!(?err, "Failed to render to texture for close animation")) else {
                return None;
            };

            let texture = TextureBuffer::from_texture(
                renderer,
                texture,
                scale.x.max(scale.y) as i32,
                Transform::Normal,
                None,
            );
            let offset = geo.loc.to_f64().to_logical(scale).to_i32_round();

            Some((texture, offset))
        }) else {
            return SolidColorRenderElement::new(
                Id::new(),
                Rectangle::from_loc_and_size(self.location, self.size)
                    .to_physical_precise_round(scale),
                CommitCounter::default(),
                [1.0, 0.0, 0.0, 1.0],
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

        let center = self.size.to_point().downscale(2);
        let origin = (center + *offset).to_physical_precise_round(scale);
        let rescale = progress * (1.0 - CLOSE_SCALE_THRESHOLD) + CLOSE_SCALE_THRESHOLD;
        let rescale = RescaleRenderElement::from_element(texture, origin, rescale);

        let location = (self.location + *offset).to_physical_precise_round(scale);
        let relocate = RelocateRenderElement::from_element(rescale, location, Relocate::Relative);
        ClosingTileRenderElement::Texture(relocate)
    }
}

fht_render_elements! {
    ClosingTileRenderElement => {
        Texture = RelocateRenderElement<RescaleRenderElement<FhtTextureElement>>,
        Solid = SolidColorRenderElement,
    }
}
