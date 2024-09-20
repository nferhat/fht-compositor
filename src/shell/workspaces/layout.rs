//! Layouts a [`Workspace`](super::Workspace) can use.
//!
//! `fht-compositor` adopts a dynamic layout system. Each workspace hold a set number of
//! [`WorkspaceTile`](super::tile::WorkspaceTile)s that are either:
//!
//! - Fullscreened, being displayed above every other tile, covering all the output
//! - Maximized, displayed about all the other tiles, but it covers the entire tiling area
//! - Tiled, and this is the state that we care about here
//!
//! The layout is responsible of taking a list of tiles and arranging them in the most optimal way
//! inside a given `tile_area`, inside two differents stacks: a master stack and a slave stack.
//!
//! 1. `nmaster`: The number of clients inside the master stack.
//! 2. `mwfact`: The proportion, after removing inner gaps, that the master stack should take of the
//!    `tile_area`
//!
//! # Acknowledgements
//!
//! These are all adaptations from [DWM's vanitygaps patch](https://dwm.suckless.org/patches/vanitygaps/)
//! with some tweaking and changes to be more idiomatic.

use std::cmp::min;
use std::ops::Mul;

use fht_compositor_config::WorkspaceLayout;
use smithay::desktop::layer_map_for_output;
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::tile::Tile;
use crate::utils::output::OutputExt;

pub struct Layout {
    // Must hold invariant that both nmaster != 0 && 0.0 < mwfact < 1.0
    nmaster: usize,
    mwfact: f32,
    output_geo: Rectangle<i32, Logical>,
    usable_geo: Rectangle<i32, Logical>,
    layouts: Vec<WorkspaceLayout>,
    active_idx: usize,
    // There's no issue if these are negative, they will just make the windows "collide" with
    // eachother. Up to the user
    inner_gaps: i32,
    outer_gaps: i32,
}

impl Layout {
    pub fn new(
        output: &Output,
        layouts: Vec<WorkspaceLayout>,
        nmaster: usize,
        mwfact: f32,
        inner_gaps: i32,
        outer_gaps: i32,
    ) -> Self {
        let output_geo = output.geometry();
        let mut layout = Self {
            nmaster,
            mwfact,
            output_geo,
            usable_geo: output_geo,
            layouts,
            active_idx: 0,
            inner_gaps,
            outer_gaps,
        };
        layout.usable_geo = layout.get_usable_geo(output);

        layout
    }

    pub fn output_resized(&mut self, output: &Output) {
        self.output_geo = output.geometry();
        self.usable_geo = self.get_usable_geo(output);
    }

    fn get_usable_geo(&self, output: &Output) -> Rectangle<i32, Logical> {
        let mut non_exclusive_zone = layer_map_for_output(output).non_exclusive_zone();
        non_exclusive_zone.loc += Point::from((self.outer_gaps, self.outer_gaps));
        non_exclusive_zone.size -= Size::from((2 * self.outer_gaps, 2 * self.outer_gaps));
        // todo: maybe add output padding in user config
        non_exclusive_zone
    }

    pub fn usable_geo(&self) -> Rectangle<i32, Logical> {
        self.usable_geo
    }

    pub fn nmaster(&self) -> usize {
        self.nmaster
    }

    pub fn mwfact(&self) -> f32 {
        self.mwfact
    }

    pub fn active(&self) -> WorkspaceLayout {
        self.layouts[self.active_idx]
    }

    pub fn select_next(&mut self) {
        let layouts_len = self.layouts.len();
        let new_active_idx = self.active_idx + 1;
        let new_active_idx = if new_active_idx == layouts_len {
            0
        } else {
            new_active_idx
        };

        self.active_idx = new_active_idx;
    }

    pub fn select_previous(&mut self) {
        let layouts_len = self.layouts.len();
        self.active_idx = match self.active_idx.checked_sub(1) {
            None => layouts_len - 1,
            Some(idx) => idx,
        };
    }

    pub fn set_layouts(&mut self, layouts: Vec<WorkspaceLayout>) {
        self.layouts = layouts;
        self.active_idx = self.active_idx.clamp(0, self.layouts.len());
    }

    pub fn set_mwfact(&mut self, mwfact: f32) {
        self.mwfact = mwfact.clamp(0.01, 0.99);
    }

    pub fn change_mwfact(&mut self, delta: f32) {
        self.mwfact = (self.mwfact + delta).clamp(0.01, 0.99);
    }

    pub fn set_nmaster(&mut self, nmaster: usize) {
        self.nmaster = nmaster.clamp(1, usize::MAX);
    }

    pub fn change_nmaster(&mut self, delta: i32) {
        self.nmaster = self
            .nmaster
            .saturating_add_signed(delta as isize)
            .clamp(1, usize::MAX);
    }

    pub fn arrange_tiles<'a>(&'a self, tiles: impl Iterator<Item = &'a mut Tile>, animate: bool) {
        let mut tiles = tiles.collect::<Vec<_>>();
        let tiles_len = tiles.len();
        let nmaster = self.nmaster;
        let mwfact = self.mwfact;
        let inner_gaps = self.inner_gaps;
        let usable_geo = self.usable_geo;

        match self.layouts[self.active_idx] {
            WorkspaceLayout::Tile => {
                let nmaster = min(nmaster, tiles_len);
                let mut master_geo @ mut stack_geo = usable_geo;

                master_geo.size.h -= (nmaster as i32).saturating_sub(1).mul(inner_gaps);
                stack_geo.size.h -= (tiles_len as i32)
                    .saturating_sub(nmaster as i32)
                    .saturating_sub(1)
                    .mul(inner_gaps);

                if tiles_len > nmaster {
                    stack_geo.size.w =
                        ((master_geo.size.w - inner_gaps) as f32 * (1.0 - mwfact)).round() as i32;
                    master_geo.size.w -= inner_gaps + stack_geo.size.w;
                    stack_geo.loc.x = master_geo.loc.x + master_geo.size.w + inner_gaps;
                };

                let master_heights = {
                    let tiles = tiles.get(0..nmaster).unwrap_or_default();
                    let cfacts = tiles.iter().map(|tile| tile.cfact()).collect::<Vec<_>>();
                    get_dimensions(&cfacts, master_geo.size.h)
                };

                let stack_heights = {
                    let tiles = tiles.get(nmaster..).unwrap_or_default();
                    let cfacts = tiles.iter().map(|tile| tile.cfact()).collect::<Vec<_>>();
                    get_dimensions(&cfacts, stack_geo.size.h)
                };

                for (idx, tile) in tiles.iter_mut().enumerate() {
                    if idx < nmaster {
                        let master_height = master_heights[idx];
                        let geo = Rectangle::from_loc_and_size(
                            master_geo.loc,
                            (master_geo.size.w, master_height),
                        );
                        tile.set_geometry(geo, animate);
                        master_geo.loc.y += master_height + inner_gaps;
                    } else {
                        let stack_height = stack_heights[idx - nmaster];
                        let new_geo = Rectangle::from_loc_and_size(
                            stack_geo.loc,
                            (stack_geo.size.w, stack_height),
                        );
                        tile.set_geometry(new_geo, animate);
                        stack_geo.loc.y += stack_height + inner_gaps;
                    }
                }
            }
            WorkspaceLayout::BottomStack => {
                let nmaster = min(nmaster, tiles_len);
                let mut master_geo @ mut stack_geo = usable_geo;

                master_geo.size.w -= (nmaster as i32).saturating_sub(1).mul(inner_gaps);
                stack_geo.size.w -= (tiles_len as i32)
                    .saturating_sub(nmaster as i32)
                    .saturating_sub(1)
                    .mul(inner_gaps);

                if tiles_len > nmaster {
                    stack_geo.size.h =
                        ((master_geo.size.h - inner_gaps) as f32 * (1.0 - mwfact)).round() as i32;
                    master_geo.size.h -= inner_gaps + stack_geo.size.h;
                    stack_geo.loc.y = master_geo.loc.y + master_geo.size.h + inner_gaps;
                };

                let master_widths = {
                    let tiles = tiles.get(0..nmaster).unwrap_or_default();
                    let cfacts = tiles.iter().map(|tile| tile.cfact()).collect::<Vec<_>>();
                    get_dimensions(&cfacts, master_geo.size.w)
                };

                let stack_widths = {
                    let tiles = tiles.get(nmaster..).unwrap_or_default();
                    let cfacts = tiles.iter().map(|tile| tile.cfact()).collect::<Vec<_>>();
                    get_dimensions(&cfacts, stack_geo.size.w)
                };

                for (idx, tile) in tiles.iter_mut().enumerate() {
                    if idx < nmaster {
                        let master_width = master_widths[idx];
                        let geo = Rectangle::from_loc_and_size(
                            master_geo.loc,
                            (master_width, master_geo.size.h),
                        );
                        tile.set_geometry(geo, animate);
                        master_geo.loc.x += master_width + inner_gaps;
                    } else {
                        let stack_width = stack_widths[idx - nmaster];
                        let geo = Rectangle::from_loc_and_size(
                            stack_geo.loc,
                            (stack_width, stack_geo.size.h),
                        );
                        tile.set_geometry(geo, animate);
                        stack_geo.loc.x += stack_width + inner_gaps;
                    }
                }
            }
            WorkspaceLayout::CenteredMaster => {
                let master_len = min(tiles_len, nmaster);
                let left_len = tiles_len.saturating_sub(nmaster) / 2;
                let right_len = (tiles_len.saturating_sub(nmaster) / 2)
                    + (tiles_len.saturating_sub(nmaster) % 2);

                let mut master_geo @ mut left_geo @ mut right_geo = usable_geo;
                master_geo.size.h -= inner_gaps * master_len.saturating_sub(1) as i32;
                left_geo.size.h -= inner_gaps * left_len.saturating_sub(1) as i32;
                right_geo.size.h -= inner_gaps * right_len.saturating_sub(1) as i32;

                if tiles_len > nmaster {
                    if (tiles_len - nmaster) > 1 {
                        master_geo.size.w =
                            ((master_geo.size.w - 2 * inner_gaps) as f32 * mwfact).round() as i32;
                        left_geo.size.w =
                            (usable_geo.size.w - master_geo.size.w - 2 * inner_gaps) / 2;
                        right_geo.size.w = usable_geo.size.w
                            - master_geo.size.w
                            - 2 * inner_gaps
                            - left_geo.size.w;
                        master_geo.loc.x += left_geo.size.w + inner_gaps;
                    } else {
                        master_geo.size.w =
                            ((master_geo.size.w - inner_gaps) as f32 * mwfact).round() as i32;
                        left_geo.size.w = 0;
                        right_geo.size.w -= master_geo.size.w - inner_gaps;
                    }

                    left_geo.loc = usable_geo.loc;
                    right_geo.loc = usable_geo.loc; // for y value only
                    right_geo.loc.x = master_geo.loc.x + master_geo.size.w + inner_gaps;
                }

                let (master_tiles, left_right_tiles) = tiles
                    .into_iter()
                    .enumerate()
                    .partition::<Vec<_>, _>(|(idx, _)| *idx < nmaster);
                let (left_tiles, right_tiles) = left_right_tiles
                    .into_iter()
                    .partition::<Vec<_>, _>(|(idx, _)| (idx.saturating_sub(nmaster) % 2) != 0);

                let left_heights = {
                    let cfacts = left_tiles
                        .iter()
                        .map(|(_, tile)| tile.cfact())
                        .collect::<Vec<_>>();
                    get_dimensions(&cfacts, left_geo.size.h)
                };
                for (tile, height) in left_tiles
                    .into_iter()
                    .map(|(_, tile)| tile)
                    .zip(left_heights)
                {
                    let geo = Rectangle::from_loc_and_size(left_geo.loc, (left_geo.size.w, height));
                    tile.set_geometry(geo, animate);
                    left_geo.loc.y += height + inner_gaps;
                }

                let master_heights = {
                    let cfacts = master_tiles
                        .iter()
                        .map(|(_, tile)| tile.cfact())
                        .collect::<Vec<_>>();
                    get_dimensions(&cfacts, master_geo.size.h)
                };
                for (tile, height) in master_tiles
                    .into_iter()
                    .map(|(_, tile)| tile)
                    .zip(master_heights)
                {
                    let geo =
                        Rectangle::from_loc_and_size(master_geo.loc, (master_geo.size.w, height));
                    tile.set_geometry(geo, animate);
                    master_geo.loc.y += height + inner_gaps;
                }

                let right_heights = {
                    let cfacts = right_tiles
                        .iter()
                        .map(|(_, tile)| tile.cfact())
                        .collect::<Vec<_>>();
                    get_dimensions(&cfacts, right_geo.size.h)
                };
                for (tile, height) in right_tiles
                    .into_iter()
                    .map(|(_, tile)| tile)
                    .zip(right_heights)
                {
                    let geo =
                        Rectangle::from_loc_and_size(right_geo.loc, (right_geo.size.w, height));
                    tile.set_geometry(geo, animate);
                    right_geo.loc.y += height + inner_gaps;
                }
            }
            WorkspaceLayout::Floating => {}
        }
    }
}

// Get the dimensions of each element partitionned on a `length `based on its `cfact``, where each
// cfact is a single element.
//
// Returns the calculated lengths for each `cfact` in `cfacts.
fn get_dimensions(cfacts: &[f32], length: i32) -> Vec<i32> {
    let total_facts: f32 = cfacts.iter().sum();
    let lengths = cfacts
        .iter()
        .map(|&cfact| (length as f32 * (cfact / total_facts)).floor() as i32)
        .collect::<Vec<_>>();
    let mut rest = lengths.iter().sum::<i32>() - length;
    lengths
        .into_iter()
        .map(|len| {
            if rest < 0 {
                rest += 1;
                len + 1
            } else if rest > 0 {
                rest -= 1;
                len - 1
            } else {
                len
            }
        })
        .collect()
}
