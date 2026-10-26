use std::cmp::{max, min};
use std::collections::BTreeSet;

use smithay::utils::{Logical, Rectangle};
use smithay::wayland::compositor::{RectangleKind, RegionAttributes};

// Ported from niri (src/utils/region.rs).

/// Convert a wl_region into a list of non-overlapping rects.
///
/// Output rects are in the same coordinate space as the region (surface-local).
pub fn region_to_non_overlapping_rects(
    region: &RegionAttributes,
    output: &mut Vec<Rectangle<i32, Logical>>,
) {
    output.clear();

    // Collect all unique Y coordinates, each band between two of them is handled separately.
    let ys = BTreeSet::from_iter(
        region
            .rects
            .iter()
            .flat_map(|(_, r)| [r.loc.y, r.loc.y + r.size.h]),
    );

    let mut ys = ys.into_iter();
    let Some(mut lo) = ys.next() else {
        // Empty region.
        return;
    };

    // Sorted list of non-overlapping [start, end) spans for the current band.
    let mut spans = Vec::<(i32, i32)>::new();

    for hi in ys {
        spans.clear();

        'region: for (kind, r) in &region.rects {
            // Skip rects that don't overlap with the band.
            if hi <= r.loc.y || r.loc.y + r.size.h <= lo {
                continue;
            }

            let mut x1 = r.loc.x;
            let mut x2 = r.loc.x + r.size.w;
            if x1 == x2 {
                continue;
            }

            match *kind {
                RectangleKind::Add => {
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

                        // Overlap, merge them.
                        spans.remove(i);
                        x1 = min(x1, start);
                        x2 = max(x2, end);
                    }

                    spans.insert(0, (x1, x2));
                }
                RectangleKind::Subtract => {
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

                        // Overlap, cut the span.
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

#[cfg(test)]
mod tests {
    use smithay::utils::Rectangle;
    use smithay::wayland::compositor::{RectangleKind, RegionAttributes};

    use super::region_to_non_overlapping_rects;

    #[test]
    fn overlapping_rects_do_not_overlap_in_output() {
        let region = RegionAttributes {
            rects: vec![
                (
                    RectangleKind::Add,
                    Rectangle::from_extremities((0, 0), (10, 10)),
                ),
                (
                    RectangleKind::Add,
                    Rectangle::from_extremities((5, 5), (15, 15)),
                ),
            ],
        };
        let mut output = Vec::new();
        region_to_non_overlapping_rects(&region, &mut output);

        // XXX: the 5x5 overlap must only be counted once.
        let area: i32 = output.iter().map(|r| r.size.w * r.size.h).sum();
        assert_eq!(area, 100 + 100 - 25);
        for (i, a) in output.iter().enumerate() {
            for b in &output[i + 1..] {
                assert!(!a.overlaps(*b));
            }
        }
    }
}
