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
//! 2. `master_width_factor`: The proportion, after removing inner gaps, that the master stack
//!    should take of the `tile_area`
//!
//! # Acknowledgements
//!
//! These are all adaptations from [DWM's vanitygaps patch](https://dwm.suckless.org/patches/vanitygaps/)
//! with some tweaking and changes to be more idiomatic.

use std::cmp::min;
use std::ops::Mul;

use serde::{Deserialize, Serialize};
use smithay::utils::{Logical, Rectangle};

use super::tile::{WorkspaceElement, WorkspaceTile};

/// All layouts [`Workspace`]s can use.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum WorkspaceLayout {
    /// The classic Master-Tile layout, also known as Master-Slave layout, or TileLeft.
    ///
    /// You have `nmaster` windows on the left side, and the other windows are in the stack, or the
    /// right side, and they share the height equally.
    ///
    /// How the master side and the stack side are proportioned is decided by the
    /// `master_width_factor` parameter, a float ranging in (0.0..1.0)
    Tile {
        nmaster: usize,
        master_width_factor: f32,
    },
    /// A twist on the [`Tile`] layout, where the master window(s) are on the top, and the stack is
    /// on the bottom half of the screen.
    ///
    /// Every logic from the [`Tile`] layout applies here, but windows share width space equally,
    /// rather than height.
    BottomStack {
        nmaster: usize,
        master_width_factor: f32,
    },
    /// The centered master layout is a layout where the master stack is in the middle and its
    /// windows are getting partitioned inside of it height-wise.
    ///
    /// The stack clients are on the left and right of the master windows, being also repartioned
    /// height-wise.
    CenteredMaster {
        nmaster: usize,
        master_width_factor: f32,
    },
    /// Floating layout, basically do nothing to arrange the windows.
    Floating,
}

impl WorkspaceLayout {
    /// Arrange workspace tiles in given `tile_area`
    ///
    /// - `tiles`: The tiles you want to arrange in `tile_area`
    /// - `tile_area`: The area you want to arrange the tiles in. You should make it local to the
    ///   workspace you are using this layout for.
    /// - `inner_gaps`: Gaps to put between tiles, these are vertical+horizontal.
    pub fn arrange_tiles<'a, E: WorkspaceElement + 'a>(
        &'a self,
        tiles: impl Iterator<Item = &'a mut WorkspaceTile<E>>,
        tile_area: Rectangle<i32, Logical>,
        inner_gaps: i32,
        animate: bool,
    ) {
        let mut tiles = tiles.collect::<Vec<_>>();
        let tiles_len = tiles.len();
        match *self {
            WorkspaceLayout::Tile {
                nmaster,
                master_width_factor: mwfact,
            } => {
                let nmaster = min(nmaster, tiles_len);
                let mut master_geo @ mut stack_geo = tile_area;

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
                    let cfacts = tiles.iter().map(|tile| tile.cfact).collect::<Vec<_>>();
                    get_dimensions(&cfacts, master_geo.size.h)
                };

                let stack_heights = {
                    let tiles = tiles.get(nmaster..).unwrap_or_default();
                    let cfacts = tiles.iter().map(|tile| tile.cfact).collect::<Vec<_>>();
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
            WorkspaceLayout::BottomStack {
                nmaster,
                master_width_factor: mwfact,
            } => {
                let nmaster = min(nmaster, tiles_len);
                let mut master_geo @ mut stack_geo = tile_area;

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
                    let cfacts = tiles.iter().map(|tile| tile.cfact).collect::<Vec<_>>();
                    get_dimensions(&cfacts, master_geo.size.w)
                };

                let stack_widths = {
                    let tiles = tiles.get(nmaster..).unwrap_or_default();
                    let cfacts = tiles.iter().map(|tile| tile.cfact).collect::<Vec<_>>();
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
            WorkspaceLayout::CenteredMaster {
                nmaster,
                master_width_factor,
            } => {
                let master_len = min(tiles_len, nmaster);
                let left_len = tiles_len.saturating_sub(nmaster) / 2;
                let right_len = (tiles_len.saturating_sub(nmaster) / 2)
                    + (tiles_len.saturating_sub(nmaster) % 2);

                let mut master_geo @ mut left_geo @ mut right_geo = tile_area;
                master_geo.size.h -= inner_gaps * master_len.saturating_sub(1) as i32;
                left_geo.size.h -= inner_gaps * left_len.saturating_sub(1) as i32;
                right_geo.size.h -= inner_gaps * right_len.saturating_sub(1) as i32;

                if tiles_len > nmaster {
                    if (tiles_len - nmaster) > 1 {
                        master_geo.size.w = ((master_geo.size.w - 2 * inner_gaps) as f32
                            * master_width_factor)
                            .round() as i32;
                        left_geo.size.w =
                            (tile_area.size.w - master_geo.size.w - 2 * inner_gaps) / 2;
                        right_geo.size.w =
                            tile_area.size.w - master_geo.size.w - 2 * inner_gaps - left_geo.size.w;
                        master_geo.loc.x += left_geo.size.w + inner_gaps;
                    } else {
                        master_geo.size.w = ((master_geo.size.w - inner_gaps) as f32
                            * master_width_factor)
                            .round() as i32;
                        left_geo.size.w = 0;
                        right_geo.size.w -= master_geo.size.w - inner_gaps;
                    }

                    left_geo.loc = tile_area.loc;
                    right_geo.loc = tile_area.loc; // for y value only
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
                        .map(|(_, tile)| tile.cfact)
                        .collect::<Vec<_>>();
                    get_dimensions(&cfacts, left_geo.size.h)
                };
                for (tile, height) in left_tiles
                    .into_iter()
                    .map(|(_, tile)| tile)
                    .zip(left_heights)
                {
                    let geo = Rectangle::from_loc_and_size(
                        left_geo.loc, (left_geo.size.w, height),
                    );
                    tile.set_geometry(geo, animate);
                    left_geo.loc.y += height + inner_gaps;
                }

                let master_heights = {
                    let cfacts = master_tiles
                        .iter()
                        .map(|(_, tile)| tile.cfact)
                        .collect::<Vec<_>>();
                    get_dimensions(&cfacts, master_geo.size.h)
                };
                for (tile, height) in master_tiles
                    .into_iter()
                    .map(|(_, tile)| tile)
                    .zip(master_heights)
                {
                    let geo = Rectangle::from_loc_and_size(
                        master_geo.loc, (master_geo.size.w, height),
                    );
                    tile.set_geometry(geo, animate);
                    master_geo.loc.y += height + inner_gaps;
                }

                let right_heights = {
                    let cfacts = right_tiles
                        .iter()
                        .map(|(_, tile)| tile.cfact)
                        .collect::<Vec<_>>();
                    get_dimensions(&cfacts, right_geo.size.h)
                };
                for (tile, height) in right_tiles
                    .into_iter()
                    .map(|(_, tile)| tile)
                    .zip(right_heights)
                {
                    let geo = Rectangle::from_loc_and_size(
                        right_geo.loc, (right_geo.size.w, height),
                    );
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
