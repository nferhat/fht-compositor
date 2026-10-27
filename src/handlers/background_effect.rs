use std::sync::{Arc, Mutex};

use smithay::backend::renderer::utils::CommitCounter;
use smithay::delegate_background_effect;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle};
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

fn blur_region(wl_surface: &WlSurface) -> (Arc<Vec<Rectangle<i32, Logical>>>, CommitCounter) {
    with_states(wl_surface, |states| {
        let Some(cache) = states.data_map.get::<BlurRegionCache>() else {
            return (Arc::default(), CommitCounter::default());
        };
        let inner = cache.0.lock().unwrap();
        (inner.rects.clone(), inner.commit)
    })
}

/// The committed blur region of `wl_surface` in physical coordinates, offset by `surface_loc`.
///
/// Without a region, `fallback` is used instead, if there is one.
pub fn surface_background_effect_region(
    wl_surface: &WlSurface,
    scale: i32,
    surface_loc: Point<i32, Physical>,
    fallback: Option<Rectangle<i32, Physical>>,
) -> (Vec<Rectangle<i32, Physical>>, CommitCounter) {
    let (rects, commit) = blur_region(wl_surface);
    if rects.is_empty() {
        return (fallback.into_iter().collect(), commit);
    }

    let rects = rects
        .iter()
        .map(|rect| {
            let mut rect = rect.to_physical_precise_round(scale);
            rect.loc += surface_loc;
            rect
        })
        .collect();
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
