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
        let mut tiles = tiles.collect::<Vec<_>>();
        match *self {
            WorkspaceLayout::Tile {
                nmaster,
                master_width_factor: mwfact,
            } => {
                let master_len = min(tiles_len, nmaster);
                let mut master_geo @ mut stack_geo = tile_area;

                master_geo.size.h -= inner_gaps * master_len.saturating_sub(1) as i32;
                stack_geo.size.h -=
                    inner_gaps * tiles_len.saturating_sub(nmaster).saturating_sub(1) as i32;

                if tiles_len > nmaster {
                    stack_geo.size.w =
                        ((master_geo.size.w - inner_gaps) as f32 * (1.0 - mwfact)).round() as i32;
                    master_geo.size.w -= inner_gaps + stack_geo.size.w;
                    stack_geo.loc.x = master_geo.loc.x + master_geo.size.w + inner_gaps;
                };

                let (mfact, sfact, mrest, srest) = get_facts(
                    tiles.as_slice(),
                    nmaster,
                    master_geo.size.h,
                    stack_geo.size.h,
                );

                for (idx, tile) in tiles.iter_mut().enumerate() {
                    if idx < nmaster {
                        let master_height = ((master_geo.size.h as f32) * (tile.cfact / mfact))
                            .round() as i32
                            + ((idx < mrest as usize) as i32);

                        let geo = Rectangle::from_loc_and_size(
                            master_geo.loc,
                            (master_geo.size.w, master_height),
                        );
                        tile.set_geometry(geo);

                        master_geo.loc.y += master_height + inner_gaps;
                    } else {
                        let stack_height = ((stack_geo.size.h as f32) * (tile.cfact / sfact))
                            .round() as i32
                            + (((idx - nmaster) < srest as usize) as i32);

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
                master_width_factor: mwfact,
            } => {
                let master_len = min(tiles_len, nmaster);
                let mut master_geo @ mut stack_geo = tile_area;

                master_geo.size.w -= inner_gaps * (master_len.saturating_sub(1)) as i32;
                stack_geo.size.w -=
                    inner_gaps * tiles_len.saturating_sub(nmaster).saturating_sub(1) as i32;

                if tiles_len > nmaster {
                    stack_geo.size.h =
                        ((master_geo.size.h - inner_gaps) as f32 * (1.0 - mwfact)).round() as i32;
                    master_geo.size.h -= inner_gaps + stack_geo.size.h;
                    stack_geo.loc.y = master_geo.loc.y + master_geo.size.h + inner_gaps;
                };

                let (mfact, sfact, mrest, srest) = get_facts(
                    tiles.as_slice(),
                    nmaster,
                    master_geo.size.h,
                    stack_geo.size.h,
                );

                for (idx, tile) in tiles.iter_mut().enumerate() {
                    if idx < nmaster {
                        let master_width = ((master_geo.size.w as f32) * (tile.cfact / mfact))
                            .round() as i32
                            + ((idx < mrest as usize) as i32);

                        let geo = Rectangle::from_loc_and_size(
                            master_geo.loc,
                            (master_width, master_geo.size.h),
                        );
                        tile.set_geometry(geo);

                        master_geo.loc.x += master_width + inner_gaps;
                    } else {
                        let stack_width = ((stack_geo.size.w as f32) * (tile.cfact / sfact)).round()
                            as i32
                            + (((idx - nmaster) < srest as usize) as i32);

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

                // Since we use three cols we cant calculate facts as usual
                let mut mfact @ mut lfact @ mut rfact = 0f32;
                for (idx, tile) in tiles.iter().enumerate() {
                    if idx < nmaster {
                        mfact += tile.cfact;
                    } else if (idx.saturating_sub(nmaster) % 2) != 0 {
                        lfact += tile.cfact;
                    } else {
                        rfact += tile.cfact;
                    }
                }

                let mut mtotal @ mut ltotal @ mut rtotal = 0;
                for (idx, tile) in tiles.iter().enumerate() {
                    if idx < nmaster {
                        mtotal += (master_geo.size.h as f32 * (tile.cfact / mfact)).round() as i32;
                    } else if (idx.saturating_sub(nmaster) % 2) != 0 {
                        ltotal += (left_geo.size.h as f32 * (tile.cfact / lfact)).round() as i32;
                    } else {
                        rtotal += (right_geo.size.h as f32 * (tile.cfact / rfact)).round() as i32;
                    }
                }

                let mrest = master_geo.size.h - mtotal;
                let lrest = left_geo.size.h - ltotal;
                let rrest = right_geo.size.h - rtotal;

                for (idx, tile) in tiles.iter_mut().enumerate() {
                    if idx < nmaster {
                        let master_height = ((master_geo.size.h as f32) * (tile.cfact / mfact))
                            .round() as i32
                            + ((idx < mrest as usize) as i32);

                        let geo = Rectangle::from_loc_and_size(
                            master_geo.loc,
                            (master_geo.size.w, master_height),
                        );
                        tile.set_geometry(geo);

                        master_geo.loc.y += master_height + inner_gaps;
                    } else if ((idx - nmaster) % 2 != 0) {
                        let left_height = ((left_geo.size.h as f32) * (tile.cfact / lfact)).round()
                            as i32
                            + (((idx.saturating_sub(2 * nmaster) as i32) < 2 * lrest) as i32);

                        let geo = Rectangle::from_loc_and_size(
                            left_geo.loc,
                            (left_geo.size.w, left_height),
                        );
                        tile.set_geometry(geo);

                        left_geo.loc.y += left_height + inner_gaps;
                    } else {
                        let right_height = ((right_geo.size.h as f32) * (tile.cfact / rfact))
                            .round() as i32
                            + (((idx.saturating_sub(2 * nmaster) as i32) < 2 * rrest) as i32);

                        let geo = Rectangle::from_loc_and_size(
                            right_geo.loc,
                            (right_geo.size.w, right_height),
                        );
                        tile.set_geometry(geo);

                        right_geo.loc.y += right_height + inner_gaps;
                    }
                }
            }
            WorkspaceLayout::Floating => {}
        }
    }
}

fn get_facts<'a, E: WorkspaceElement + 'a>(
    tiles: &'a [&'a mut WorkspaceTile<E>],
    nmaster: usize,
    msize: i32,
    ssize: i32,
) -> (f32, f32, i32, i32) {
    let mut mfacts @ mut sfacts = 0f32;
    let mut mtotal @ mut stotal = 0i32;

    for (idx, tile) in tiles.iter().enumerate() {
        if idx < nmaster {
            mfacts += tile.cfact
        } else {
            sfacts += tile.cfact
        }
    }

    for (idx, tile) in tiles.iter().enumerate() {
        if idx < nmaster {
            mtotal += (msize as f32 * (tile.cfact / mfacts)).round() as i32;
        } else {
            stotal += (ssize as f32 * (tile.cfact / sfacts)).round() as i32;
        }
    }

    (mfacts, sfacts, msize - mtotal, ssize - stotal)
}
