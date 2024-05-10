use std::cmp::min;

use serde::{Deserialize, Serialize};
use smithay::utils::Rectangle;

use super::tile::{WorkspaceElement, WorkspaceTile};
use crate::utils::geometry::Local;

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
        tiles_len: usize,
        tile_area: Rectangle<i32, Local>,
        inner_gaps: i32,
    ) {
        match *self {
            WorkspaceLayout::Tile {
                nmaster,
                master_width_factor,
            } => {
                // A lone master window in a workspace will basically appear the same as a
                // maximized window, so it's logical to start from there
                let master_len = min(tiles_len, nmaster);
                let mut master_geo = tile_area;
                // If there's n master clients, there's (n-1) gets to leave between them
                master_geo.size.h -= inner_gaps * (master_len.saturating_sub(1)) as i32;
                // Divide and use floor.
                master_geo.size.h = (master_geo.size.h as f64 / master_len as f64).floor() as i32;
                // Calculate the rest of the height to add for each master client.
                // Using floor will always leave us with some removed remainder, so we account for
                // it here.
                let mut master_rest = tile_area.size.h
                    - (master_len.saturating_sub(1) as i32 * inner_gaps)
                    - (master_len as i32 * master_geo.size.h);

                // Same logic for stack, try to account for rest
                let stack_len = tiles_len.saturating_sub(nmaster);
                let mut stack_geo = tile_area;
                let mut stack_rest = 0;
                stack_geo.size.h -= inner_gaps * stack_len.saturating_sub(1) as i32;
                if tiles_len > nmaster {
                    master_geo.size.w = tile_area.size.w - inner_gaps;
                    master_geo.size.w =
                        (master_geo.size.w as f32 * master_width_factor).round() as i32;

                    // Stack uses remiander of master geo in width.
                    stack_geo.size.w -= master_geo.size.w + inner_gaps;
                    stack_geo.loc.x += master_geo.size.w + inner_gaps;
                    stack_geo.size.h = (stack_geo.size.h as f64 / stack_len as f64).floor() as i32;
                    stack_rest = tile_area.size.h
                        - (stack_len.saturating_sub(1) as i32 * inner_gaps)
                        - (stack_len as i32 * stack_geo.size.h);
                };

                for (idx, tile) in tiles.enumerate() {
                    if idx < nmaster {
                        let mut master_height = master_geo.size.h;
                        if master_rest != 0 {
                            master_height += 1;
                            master_rest -= 1;
                        }

                        let geo = Rectangle::from_loc_and_size(
                            master_geo.loc,
                            (master_geo.size.w, master_height),
                        );
                        tile.set_geometry(geo);

                        master_geo.loc.y += master_height + inner_gaps;
                    } else {
                        let mut stack_height = stack_geo.size.h;
                        if stack_rest != 0 {
                            stack_height += 1;
                            stack_rest -= 1;
                        }

                        let new_geo = Rectangle::from_loc_and_size(
                            stack_geo.loc,
                            (stack_geo.size.w, stack_height),
                        );
                        tile.set_geometry(new_geo);

                        stack_geo.loc.y += stack_height + inner_gaps;
                    }
                }
            }
            WorkspaceLayout::BottomStack {
                nmaster,
                master_width_factor,
            } => {
                // A lone master window in a workspace will basically appear the same as a
                // maximized window, so it's logical to start from there
                let master_len = min(tiles_len, nmaster);
                let mut master_geo = tile_area;
                // If there's n master clients, there's (n-1) gets to leave between them
                master_geo.size.w -= inner_gaps * (master_len.saturating_sub(1)) as i32;
                // Divide and use floor.
                master_geo.size.w = (master_geo.size.w as f64 / master_len as f64).floor() as i32;
                // Calculate the rest of the height to add for each master client.
                // Using floor will always leave us with some removed remainder, so we account for
                // it here.
                let mut master_rest = tile_area.size.w
                    - (master_len.saturating_sub(1) as i32 * inner_gaps)
                    - (master_len as i32 * master_geo.size.w);

                // Same logic for stack, try to account for rest
                let stack_len = tiles_len.saturating_sub(nmaster);
                let mut stack_geo = tile_area;
                let mut stack_rest = 0;
                stack_geo.size.w -= inner_gaps * stack_len.saturating_sub(1) as i32;
                if tiles_len > nmaster {
                    master_geo.size.h = tile_area.size.h - inner_gaps;
                    master_geo.size.h =
                        (master_geo.size.h as f32 * master_width_factor).round() as i32;

                    // Stack uses remiander of master geo in width.
                    stack_geo.size.h -= master_geo.size.h + inner_gaps;
                    stack_geo.loc.y += master_geo.size.h + inner_gaps;
                    stack_geo.size.w = (stack_geo.size.w as f64 / stack_len as f64).floor() as i32;
                    stack_rest = tile_area.size.w
                        - (stack_len.saturating_sub(1) as i32 * inner_gaps)
                        - (stack_len as i32 * stack_geo.size.w);
                };

                for (idx, tile) in tiles.enumerate() {
                    if idx < nmaster {
                        let mut master_width = master_geo.size.w;
                        if master_rest != 0 {
                            master_width += 1;
                            master_rest -= 1;
                        }

                        let geo = Rectangle::from_loc_and_size(
                            master_geo.loc,
                            (master_width, master_geo.size.h),
                        );
                        tile.set_geometry(geo);

                        master_geo.loc.x += master_width + inner_gaps;
                    } else {
                        let mut stack_width = stack_geo.size.w;
                        if stack_rest != 0 {
                            stack_width += 1;
                            stack_rest -= 1;
                        }

                        let geo = Rectangle::from_loc_and_size(
                            stack_geo.loc,
                            (stack_width, stack_geo.size.h),
                        );
                        tile.set_geometry(geo);

                        stack_geo.loc.x += stack_width + inner_gaps;
                    }
                }
            }
            #[allow(unused)]
            WorkspaceLayout::CenteredMaster {
                nmaster,
                master_width_factor,
            } => {
                // A lone master window in a workspace will basically appear the same as a
                // maximized window, so it's logical to start from there
                let master_len = min(tiles_len, nmaster);
                let mut master_geo = tile_area;
                // If there's n master clients, there's (n-1) gets to leave between them
                master_geo.size.h -= inner_gaps * (master_len.saturating_sub(1)) as i32;
                // Divide and use floor.
                master_geo.size.h = (master_geo.size.h as f64 / master_len as f64).floor() as i32;
                // Calculate the rest of the height to add for each master client.
                // Using floor will always leave us with some removed remainder, so we account for
                // it here.
                let mut master_rest = tile_area.size.h
                    - (master_len.saturating_sub(1) as i32 * inner_gaps)
                    - (master_len as i32 * master_geo.size.h);

                // Repeat for left column.
                let left_len = tiles_len.saturating_sub(nmaster) / 2;
                let mut left_geo = Rectangle::default();
                left_geo.size.h =
                    tile_area.size.h - (inner_gaps * left_len.saturating_sub(1) as i32);
                left_geo.size.h = (left_geo.size.h as f64 / left_len as f64).floor() as i32;
                let mut left_rest = tile_area.size.h
                    - (left_len.saturating_sub(1) as i32 * inner_gaps)
                    - (left_len as i32 * left_geo.size.h);

                // Repeat again for right column
                let right_len = (tiles_len.saturating_sub(nmaster) / 2) as i32
                    + (tiles_len.saturating_sub(nmaster) % 2) as i32;
                let mut right_geo = Rectangle::default();
                right_geo.size.h =
                    tile_area.size.h - (inner_gaps * right_len.saturating_sub(1) as i32);
                right_geo.size.h = (right_geo.size.h as f64 / right_len as f64).floor() as i32;
                let mut right_rest = tile_area.size.h
                    - (right_len.saturating_sub(1) as i32 * inner_gaps)
                    - (right_len as i32 * right_geo.size.h);

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
                        right_geo.size.w = master_geo.size.w - inner_gaps;
                    }

                    left_geo.loc = tile_area.loc;
                    right_geo.loc = tile_area.loc; // for y value only
                    right_geo.loc.x = master_geo.loc.x + master_geo.size.w + inner_gaps;
                }

                for (idx, tile) in tiles.enumerate() {
                    if idx < nmaster {
                        let mut master_height = master_geo.size.h;
                        if master_rest != 0 {
                            master_height += 1;
                            master_rest -= 1;
                        }

                        let geo = Rectangle::from_loc_and_size(
                            master_geo.loc,
                            (master_geo.size.w, master_height),
                        );
                        tile.set_geometry(geo);

                        master_geo.loc.y += master_geo.size.h + inner_gaps;
                    } else if ((idx - nmaster) % 2 != 0) {
                        let mut left_height = left_geo.size.h;
                        if left_rest != 0 {
                            left_height += 1;
                            left_rest -= 1;
                        }

                        let geo = Rectangle::from_loc_and_size(
                            left_geo.loc,
                            (left_geo.size.w, left_height),
                        );
                        tile.set_geometry(geo);

                        left_geo.loc.y += left_geo.size.h + inner_gaps;
                    } else {
                        let mut right_height = right_geo.size.h;
                        if right_rest != 0 {
                            right_height += 1;
                            right_rest -= 1;
                        }

                        let geo = Rectangle::from_loc_and_size(
                            right_geo.loc,
                            (right_geo.size.w, right_height),
                        );
                        tile.set_geometry(geo);

                        right_geo.loc.y += right_geo.size.h + inner_gaps;
                    }
                }
            }
            WorkspaceLayout::Floating => {}
        }
    }
}
