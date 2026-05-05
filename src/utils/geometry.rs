use std::cmp::{max, min};
use std::collections::BTreeSet;

use smithay::utils::{Logical, Rectangle};
use smithay::wayland::compositor::{RectangleKind, RegionAttributes};

/// Converts a `wl_region` (aka smithay's [`RegionAttributes`]) into a set of non-overlapping
/// rectangles, pushing them into the `output` vector.
///
/// Credit to niri for this function!
pub fn region_to_non_overlapping_rects(
    region: &RegionAttributes,
    output: &mut Vec<Rectangle<i32, Logical>>,
) {
    crate::profile_function!();

    output.clear();

    // Collect all unique Y coordinates.
    let ys = BTreeSet::from_iter(
        region
            .rects
            .iter()
            .flat_map(|(_, r)| [r.loc.y, r.loc.y + r.size.h]),
    );

    let mut ys = ys.into_iter();
    let Some(mut lo) = ys.next() else {
        // The region was empty.
        return;
    };

    // Sorted list of non-overlapping [start, end) tuples.
    let mut spans = Vec::<(i32, i32)>::new();

    // Iterate over Y bands.
    for hi in ys {
        spans.clear();

        'region: for (kind, r) in &region.rects {
            // Skip rects that don't overlap with the Y band.
            if hi <= r.loc.y || r.loc.y + r.size.h <= lo {
                continue;
            }

            let mut x1 = r.loc.x;
            let mut x2 = r.loc.x + r.size.w;
            if x1 == x2 {
                // Empty rect.
                continue;
            }

            match *kind {
                RectangleKind::Add => {
                    // Iterate over existing spans backwards.
                    for i in (0..spans.len()).rev() {
                        let (start, end) = spans[i];

                        // New span is to the right.
                        if end < x1 {
                            spans.insert(i + 1, (x1, x2));
                            continue 'region;
                        }

                        // New span is to the left.
                        if x2 < start {
                            continue;
                        }

                        // New span overlaps this span; merge them.
                        spans.remove(i);
                        x1 = min(x1, start);
                        x2 = max(x2, end);
                    }

                    spans.insert(0, (x1, x2));
                }
                RectangleKind::Subtract => {
                    // Iterate over existing spans backwards.
                    for i in (0..spans.len()).rev() {
                        let (start, end) = spans[i];

                        // Subtract span is to the right.
                        if end <= x1 {
                            continue 'region;
                        }

                        // Subtract span is to the left.
                        if x2 <= start {
                            continue;
                        }

                        // Subtract span overlaps this span.
                        spans.remove(i);
                        if x2 < end {
                            spans.insert(i, (x2, end));
                        }
                        if start < x1 {
                            spans.insert(i, (start, x1));
                        }
                    }
                }
            }
        }

        for (x1, x2) in spans.drain(..) {
            output.push(Rectangle::from_extremities((x1, lo), (x2, hi)));
        }

        lo = hi;
    }
}
