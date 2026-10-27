use std::sync::{Arc, Mutex};

use smithay::backend::renderer::utils::CommitCounter;
use smithay::delegate_background_effect;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale};
use smithay::wayland::background_effect::{
    BackgroundEffectSurfaceCachedState, ExtBackgroundEffectHandler,
};
use smithay::wayland::compositor::{add_post_commit_hook, with_states, RegionAttributes};

use crate::state::State;
use crate::utils::region::region_to_non_overlapping_rects;

/// The blur region of a surface, cached in its data map.
///
/// The rects are relative to the surface. They are only recomputed on commit, since a region
/// request is only pending until the surface commits it.
#[derive(Default)]
struct BlurRegionCache(Mutex<Inner>);

#[derive(Default)]
struct Inner {
    /// Whether the region was requested, and has to be read on the next commit.
    pending: bool,
    rects: Arc<Vec<Rectangle<i32, Logical>>>,
    /// Bumped whenever the region changes, so the blur elements get redrawn.
    commit: CommitCounter,
}

/// Adds the post commit hook that updates the blur region of `wl_surface` once it commits.
///
/// The hook does nothing for surfaces that never requested a blur region, so it can be added to
/// every window and layer surface.
pub fn add_blur_region_hook(wl_surface: &WlSurface) {
    add_post_commit_hook::<State, _>(wl_surface, |_state, _dh, surface| {
        with_states(surface, |states| {
            let Some(cache) = states.data_map.get::<BlurRegionCache>() else {
                return;
            };
            let mut inner = cache.0.lock().unwrap();
            if !std::mem::take(&mut inner.pending) {
                return;
            }

            let mut cached = states
                .cached_state
                .get::<BackgroundEffectSurfaceCachedState>();
            let mut rects = Vec::new();
            if let Some(region) = &cached.current().blur_region {
                region_to_non_overlapping_rects(region, &mut rects);
            }

            inner.rects = Arc::new(rects);
            inner.commit.increment();
        });
    });
}

fn mark_pending(wl_surface: &WlSurface) {
    with_states(wl_surface, |states| {
        let cache = states
            .data_map
            .get_or_insert_threadsafe(BlurRegionCache::default);
        cache.0.lock().unwrap().pending = true;
    });
}

/// Access the cached blur region state.
fn with_cached_blur_region_state<F, T>(wl_surface: &WlSurface, f: F) -> T
where
    F: FnOnce(&Inner) -> T,
{
    with_states(wl_surface, |states| {
        let cached = states.data_map.get_or_insert(BlurRegionCache::default);
        let inner = cached.0.lock().unwrap();
        f(&*inner)
    })
}

/// The committed blur region of `wl_surface` in physical coordinates, offset by `surface_loc`.
///
/// `stretch` scales the rects around the surface origin, first, for windows that are being resized.
/// Without a region, `fallback` is used instead, if there is one. The rects are cropped to `crop`
/// afterwards, so they never go past it.
pub fn surface_background_effect_region(
    wl_surface: &WlSurface,
    scale: i32,
    surface_loc: Point<i32, Physical>,
    stretch: Scale<f64>,
    crop: Rectangle<i32, Physical>,
    fallback: Option<Rectangle<i32, Physical>>,
) -> (Vec<Rectangle<i32, Physical>>, CommitCounter) {
    let (rects, commit) = with_cached_blur_region_state(wl_surface, |cached_state| {
        (cached_state.rects.clone(), cached_state.commit)
    });

    let rects: Vec<_> = if rects.is_empty() {
        fallback.into_iter().collect()
    } else {
        rects
            .iter()
            .map(|r| r.to_physical_precise_round::<_, i32>(scale))
            .map(|r| r.to_f64().upscale(stretch).to_i32_round())
            .map(|mut rect| {
                rect.loc += surface_loc;
                rect
            })
            // Intersect on physical coordinates since its what we will be rendering with.
            .filter_map(|rect| rect.intersection(crop))
            .collect()
    };

    (rects, commit)
}

impl ExtBackgroundEffectHandler for State {
    fn set_blur_region(&mut self, wl_surface: WlSurface, _region: RegionAttributes) {
        mark_pending(&wl_surface);
    }

    fn unset_blur_region(&mut self, wl_surface: WlSurface) {
        mark_pending(&wl_surface);
    }
}

delegate_background_effect!(State);
