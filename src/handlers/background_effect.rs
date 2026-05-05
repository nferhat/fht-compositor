use std::sync::{Arc, Mutex, MutexGuard};

use smithay::delegate_background_effect;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::background_effect::{Capability, ExtBackgroundEffectHandler};
use smithay::wayland::compositor::{with_states, RegionAttributes, SurfaceData};

use crate::state::State;
use crate::utils::geometry::region_to_non_overlapping_rects;

impl ExtBackgroundEffectHandler for State {
    fn capabilities(&self) -> Capability {
        Capability::Blur
    }

    fn set_blur_region(&mut self, wl_surface: WlSurface, region: RegionAttributes) {
        // Here we recompute blur data on-the-spot. Niri delays this until the tile gets renderered
        // (since a tile can be offscreen forever with scrolling), however we don't concern
        // with this, since a tile will most likely always get shown.
        with_states(&wl_surface, |states| {
            let mut guard = CachedBackgroundEffectState::get(states);
            let rects = if let Some(arc) = &mut guard.rects {
                if Arc::strong_count(arc) > 1 {
                    debug!("cloning rects due to non-unique reference");
                }
                arc
            } else {
                guard.rects.insert(Arc::new(Vec::new()))
            };
            let rects = Arc::make_mut(rects);

            region_to_non_overlapping_rects(&region, rects);
        });
    }

    fn unset_blur_region(&mut self, wl_surface: WlSurface) {
        with_states(&wl_surface, |states| {
            let mut guard = CachedBackgroundEffectState::get(states);
            _ = guard.rects.take();
        });
    }
}

#[derive(Default)]
struct CachedBackgroundEffectState {
    /// Cached non-overlapped rects in surface-local coordinates.
    // FIXME: Maybe use tinyvec similar to DamageBag and such
    rects: Option<Arc<Vec<Rectangle<i32, Logical>>>>,
}

impl CachedBackgroundEffectState {
    pub fn get<'a>(states: &'a SurfaceData) -> MutexGuard<'a, Self> {
        states
            .data_map
            .get_or_insert_threadsafe(Mutex::<Self>::default)
            .lock()
            .unwrap()
    }
}

/// Gets the cached blur region of a surface, requested by `ext-background-effect-v1` protocol.
///
/// If this is `None`, the surface does not want a blur region associated with itself.
pub fn get_cached_blur_region(states: &SurfaceData) -> Option<Arc<Vec<Rectangle<i32, Logical>>>> {
    let guard = CachedBackgroundEffectState::get(states);
    guard.rects.clone()
}

delegate_background_effect!(State);
