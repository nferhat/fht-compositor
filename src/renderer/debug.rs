use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::Color32F;

use crate::renderer::{FhtRenderElement, FhtRenderer};

const DAMAGE_COLOR: Color32F = Color32F::new(0.3, 0.0, 0.0, 0.3);
const OPAQUE_REGION_COLOR: Color32F = Color32F::new(0.0, 0.0, 0.3, 0.3);
const SEMITRANSPARENT_COLOR: Color32F = Color32F::new(0.0, 0.3, 0.0, 0.3);

crate::fht_render_elements! {
    DebugRenderElement => {
        Solid = SolidColorRenderElement,
    }
}

pub fn draw_damage<R: FhtRenderer>(
    damage_tracker: &mut OutputDamageTracker,
    elements: &mut Vec<FhtRenderElement<R>>,
) {
    let _span = tracy_client::span!("draw_damage");
    let Ok((Some(damage), _)) = damage_tracker.damage_output(1, elements) else {
        return;
    };

    for &damage_rect in damage {
        let color = SolidColorRenderElement::new(
            Id::new(),
            damage_rect,
            CommitCounter::default(),
            DAMAGE_COLOR,
            Kind::Unspecified,
        );
        elements.insert(0, DebugRenderElement::Solid(color).into());
    }
}

pub fn push_opaque_regions<R: FhtRenderer>(
    elem: &FhtRenderElement<R>,
    scale: i32,
    push: &mut dyn FnMut(FhtRenderElement<R>),
) {
    // HACK
    if format!("{elem:?}").contains("ExtraDamage") {
        return;
    }

    let scale = (scale as f64).into();
    let geo = elem.geometry(scale);
    let mut opaque = elem.opaque_regions(scale).to_vec();

    for rect in &mut opaque {
        rect.loc += geo.loc;
    }

    let semitransparent = geo.subtract_rects(opaque.iter().copied());

    for rect in opaque {
        let color = SolidColorRenderElement::new(
            Id::new(),
            rect,
            CommitCounter::default(),
            OPAQUE_REGION_COLOR,
            Kind::Unspecified,
        );
        push(DebugRenderElement::from(color).into());
    }

    for rect in semitransparent {
        let color = SolidColorRenderElement::new(
            Id::new(),
            rect,
            CommitCounter::default(),
            SEMITRANSPARENT_COLOR,
            Kind::Unspecified,
        );
        push(DebugRenderElement::from(color).into());
    }
}
