use std::cmp::min;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fht_compositor_config::{InsertWindowStrategy, WorkspaceLayout};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::output::Output;
use smithay::utils::{IsAlive, Logical, Point, Rectangle, Size};

use super::closing_tile::{ClosingTile, ClosingTileRenderElement};
use super::tile::{Tile, TileRenderElement};
use super::Config;
use crate::fht_render_elements;
use crate::renderer::FhtRenderer;
use crate::utils::output::OutputExt;
use crate::window::Window;

static WORKSPACE_IDS: AtomicUsize = AtomicUsize::new(0);

/// Identifier of a [`Workspace`].
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct WorkspaceId(usize);
impl WorkspaceId {
    /// Create a unique [`WorkspaceId`].
    ///
    /// Panics when you create [`usize::MAX - 1`] items.
    fn unique() -> Self {
        Self(WORKSPACE_IDS.fetch_add(1, Ordering::SeqCst))
    }
}
impl std::fmt::Debug for WorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "workspace-{}", self.0)
    }
}

#[derive(Debug)]
pub struct Workspace {
    /// The unique ID of this workspace.
    id: WorkspaceId,

    /// The workspace index inside its parent [`Monitor`](super::Monitor).
    index: usize,

    /// The output associated with the Monitor of this workspace.
    output: Output,

    /// The [`Tile`]s of this workspace.
    tiles: Vec<Tile>,

    /// The [`ClosingTile`]s in this workspace.
    ///
    /// When a [`Window`] closes, the [`Tile`] gets turned into a potential [`ClosingTile`] (if
    /// window open/close animation is enabled), and gets rendered the last place of the [`Tile`]
    /// while it fades out and then gets cleaned from this vector.
    closing_tiles: Vec<ClosingTile>,

    /// The active [`Tile`] index. Must be < tiles.len()
    ///
    /// If `tiles.len() == 0`, this is [`None`]
    active_tile_idx: Option<usize>,

    /// The fullscreen tile index.
    ///
    /// Workspace fullscreening is exclusive, IE. only one tile can be fullscreened at a time.
    ///
    /// If any action regarding this workspace is being done (for example changing focus, inserting
    /// a new window, the fullscreen dies), this fullscreen gets removed.
    fullscreened_tile_idx: Option<usize>,

    /// The list of layouts of this workspace.
    ///
    /// These will be used in order to arrange [`Tile`]s in the [`Workspace`].
    ///
    /// This must NEVER be empty.
    layouts: Vec<WorkspaceLayout>,

    /// The index of the active layout.
    active_layout_idx: usize,

    /// The master width factor.
    ///
    /// It is used in order to determine how much screen real estate should the master take up,
    /// relative to the slave stack.
    mwfact: f64,

    /// The number of clients in the master stack.
    ///
    /// This must NEVER be 0.
    nmaster: usize,

    /// The gaps of this workspace.
    ///
    /// The gaps are in the following order:
    /// - `gaps.0`: outer gaps, around the screen edge.
    /// - `gaps.1`: inner gaps, between [`Tile`]s
    gaps: (i32, i32),

    /// Whether this [`Workspace`] has transient layout changes.
    ///
    /// When the user applies changes to the [`Workspace`] layout settings, for example using
    /// [`Workspace::select_next_layout`], [`Workspace::set_mwfact`], etc..., we do not want to
    /// overrides these settings again when reloading the configuration, as this leads to a janky
    /// user experience.
    has_transient_layout_changes: bool,

    /// Shared configuration of the workspace system
    pub config: Rc<Config>,
}

impl Workspace {
    /// Create a new [`Workspace`] on this [`Output`].
    pub fn new(output: Output, index: usize, config: &Rc<Config>) -> Self {
        Self {
            id: WorkspaceId::unique(),
            index,
            output,
            tiles: vec![],
            closing_tiles: vec![],
            active_tile_idx: None,
            fullscreened_tile_idx: None,
            layouts: config.layouts.clone(),
            active_layout_idx: 0,
            mwfact: config.mwfact,
            nmaster: config.nmaster,
            gaps: config.gaps,
            has_transient_layout_changes: false,
            config: Rc::clone(config),
        }
    }

    /// Get the [`Output`] associated with this [`Workspace`].
    pub fn output(&self) -> &Output {
        &self.output
    }

    /// Get the [`WorkspaceId`] associated with this [`Workspace`].
    pub fn id(&self) -> WorkspaceId {
        self.id
    }

    /// Get the index of this [`Workspace`] in its parent [`Monitor`](super::Monitor).
    pub fn index(&self) -> usize {
        self.index
    }

    /// Merge this [`Workspace`] with another one.
    pub fn merge_with(&mut self, mut other: Self) {
        if let Some(other_fullscreen_idx) = other.fullscreened_tile_idx.take() {
            // Current behaviour is to drop the current fullscreen status in both the current and
            // the other workspace in other to not silently (without the user's knowledge) merge
            // them.
            other.tiles[other_fullscreen_idx]
                .window()
                .request_fullscreen(false);
        }

        if let Some(fullscreen_idx) = self.fullscreened_tile_idx.take() {
            self.tiles[fullscreen_idx]
                .window()
                .request_fullscreen(false);
        }

        for window in other.tiles.into_iter().map(Tile::into_window) {
            self.insert_window(window, true);
        }
    }

    /// Reload the configuration of this [`Workspace`].
    pub fn reload_config(&mut self, config: &Rc<Config>) {
        // Reload the shared Rcs with workspace system config.
        self.config = Rc::clone(config);
        for tile in &mut self.tiles {
            tile.config = Rc::clone(config);
        }

        // Workspace-specific layout changes.

        // These are only the layout parameters, layout list still gets updated as usual.
        self.layouts = config.layouts.clone();
        self.active_layout_idx = self.active_layout_idx.clamp(0, self.layouts.len() - 1);

        // Gaps are purely visual, they should do not affect the layout much...
        self.gaps = config.gaps;

        if !self.has_transient_layout_changes {
            self.mwfact = config.mwfact;
            self.nmaster = config.nmaster;
        }

        self.arrange_tiles(true);
        self.refresh();
    }

    /// Run periodic clean-up tasks.
    pub fn refresh(&mut self) {
        let mut arrange = false;

        // FIXME: This causes to always re-arrange???
        // if self
        //     .fullscreened_tile_idx
        //     .as_ref()
        //     .is_some_and(|&idx| self.tiles.get(idx).is_none())
        // {
        //     dbg!("arrange cause not fullscreen");
        //     // Fullscreen tile idx points to non-existent tile!?
        //     // This should never happen in practice but still handle this edge case.
        //     let _ = self.fullscreened_tile_idx.take();
        //     arrange = true;
        // }

        if self
            .fullscreened_tile_idx
            .as_ref()
            .is_some_and(|&fs_idx| Some(fs_idx) != self.active_tile_idx)
        {
            // Two possible cases:
            // - We changed focus while there's a fullscreen tile
            // - The tile order changed.
            // Both of these warrant a layout arrange.
            arrange = true;
        }

        if self
            .fullscreened_tile_idx
            .take_if(|&mut idx| !self.tiles[idx].window().alive())
            .is_some()
        {
            // The previous fullscreen is dead, arrange as a heuristic move
            arrange = true;
        }

        if self
            .fullscreened_tile_idx
            .as_ref()
            .is_some_and(|&idx| !self.tiles[idx].window().fullscreen())
        {
            // The current fullscreened tile window is not fullscreened anymore.
            //
            // This can be caused by user interaction inside the window, for example a unfullscreen
            // button, or a state toggle.
            //
            // This can also be triggered by other parts of the compositor logic, assuming that we
            // (the workspace) will take care of unfullscreening the window.
            arrange = true;
        }

        let output_geometry = self.output.geometry();
        if let Some(fullscreened_tile) = self
            .fullscreened_tile_idx
            .as_ref()
            .map(|&idx| &mut self.tiles[idx])
        {
            Self::refresh_window(
                &self.output,
                output_geometry,
                fullscreened_tile,
                true, // Fullscreen window gets exclusive activation and focus.
            );
        }

        // Clean zombies.
        // Cleaning fullscreen zombie case has been handled above.
        self.tiles.retain(|tile| {
            if !tile.window().alive() {
                arrange = true; // we removed a tile, layout WILL change.
                return false;
            }

            true
        });

        if !self.tiles.is_empty() {
            if let Some(active_idx) = &mut self.active_tile_idx {
                // Avoid out-of-bounds access
                *active_idx = (*active_idx).clamp(0, self.tiles.len().saturating_sub(1));
            }
        } else {
            self.active_tile_idx = None;
            return;
        }

        if arrange {
            self.arrange_tiles(true);
        }

        for (idx, tile) in self.tiles.iter_mut().enumerate() {
            Self::refresh_window(
                &self.output,
                output_geometry,
                tile,
                Some(idx) == self.active_tile_idx,
            );
        }
    }

    /// Handle a refresh for a window.
    fn refresh_window(
        output: &Output,
        output_geometry: Rectangle<i32, Logical>,
        tile: &mut Tile,
        active: bool,
    ) {
        let window = tile.window();
        window.request_activated(active);

        let mut bbox = window.bbox();
        bbox.loc = tile.location() + tile.window_loc() + output_geometry.loc;
        if let Some(mut overlap) = output_geometry.intersection(bbox) {
            // overlap must be in window-local coordinates.
            overlap.loc -= bbox.loc;
            window.enter_output(output, overlap);
        }

        window.send_pending_configure();
        window.refresh();
    }

    /// Get the [`Workspace`]'s active [`Tile`] index, if any.
    pub fn active_tile_idx(&self) -> Option<usize> {
        self.active_tile_idx
    }

    /// Set the [`Workspace`]'s active [`Tile`] index, if any.
    ///
    /// This will get clamped to a valid value when [`Workspace::refresh`] is called.
    pub fn set_active_tile_idx(&mut self, idx: usize) {
        self.remove_current_fullscreen();
        self.active_tile_idx = Some(idx);
    }

    /// Activate the [`Tile`] that comes next in the [`Workspace`].
    ///
    /// If the active [`Tile`] is the last, this function cycles back to the first one.
    pub fn activate_next_tile(&mut self, animate: bool) -> Option<Window> {
        if self.tiles.len() < 2 {
            return None;
        }
        self.remove_current_fullscreen();

        // SAFETY: self.active_tile_idx is always some since self.tiles.len() >= 2
        self.active_tile_idx = Some(match self.active_tile_idx.unwrap() + 1 {
            // We were on the last tile, cycle back.
            idx if idx == self.tiles.len() => 0,
            idx => idx,
        });
        self.arrange_tiles(animate);
        self.active_window()
    }

    /// Activate the [`Tile`] that comes previous in the [`Workspace`].
    ///
    /// If the active [`Tile`] is the first, this function cycles back to the last one.
    pub fn activate_previous_tile(&mut self, animate: bool) -> Option<Window> {
        if self.tiles.len() < 2 {
            return None;
        }
        self.remove_current_fullscreen();

        // SAFETY: self.active_tile_idx is always some since self.tiles.len() >= 2
        self.active_tile_idx = Some(match self.active_tile_idx.unwrap().checked_sub(1) {
            // We were on the last tile, cycle back.
            None => self.tiles.len() - 1,
            Some(idx) => idx,
        });
        self.arrange_tiles(animate);
        self.active_window()
    }

    /// Swaps the currently active [`Tile`] with the next one.
    pub fn swap_active_tile_with_next(&mut self, keep_focus: bool, animate: bool) -> bool {
        if self.tiles.len() < 2 {
            return false;
        }
        self.remove_current_fullscreen();

        // SAFETY: self.active_tile_idx is always some since self.tiles.len() >= 2
        let active_idx = self.active_tile_idx.unwrap();
        let next_idx = match active_idx + 1 {
            idx if idx == self.tiles.len() => 0,
            idx => idx,
        };
        if keep_focus {
            self.active_tile_idx = Some(next_idx);
        }
        self.tiles.swap(active_idx, next_idx);
        self.arrange_tiles(animate);
        true
    }

    /// Swaps the currently active [`Tile`] with the previous one.
    pub fn swap_active_tile_with_previous(&mut self, keep_focus: bool, animate: bool) -> bool {
        if self.tiles.len() < 2 {
            return false;
        }
        self.remove_current_fullscreen();

        // SAFETY: self.active_tile_idx is always some since self.tiles.len() >= 2
        let active_idx = self.active_tile_idx.unwrap();
        let prev_idx = match active_idx.checked_sub(1) {
            None => self.tiles.len() - 1,
            Some(idx) => idx,
        };
        if keep_focus {
            self.active_tile_idx = Some(prev_idx);
        }
        self.tiles.swap(active_idx, prev_idx);
        self.arrange_tiles(animate);
        true
    }

    /// Get the [`Workspace`]'s active [`Window`] index, if any.
    pub fn active_window(&self) -> Option<Window> {
        self.tiles
            .get(self.active_tile_idx?)
            .map(Tile::window)
            .cloned()
    }

    /// Get a reference to the the [`Workspace`]'s active [`Tile`].
    pub fn active_tile(&self) -> Option<&Tile> {
        self.tiles.get(self.active_tile_idx?)
    }

    /// Get a mutable reference to the the [`Workspace`]'s active [`Tile`].
    pub fn active_tile_mut(&mut self) -> Option<&mut Tile> {
        self.tiles.get_mut(self.active_tile_idx?)
    }

    /// Get an iterator of the [`Workspace`]'s [`Window`]s
    ///
    /// This includes the fullscreened [`Window`], if any.
    pub fn windows(&self) -> impl Iterator<Item = &Window> + ExactSizeIterator {
        self.tiles.iter().map(Tile::window)
    }

    /// Get an iterator over the [`Workspace`]'s [`Tile`]s.
    ///
    /// This includes the fullscreened [`Tile`], if any.
    pub fn tiles(&self) -> impl Iterator<Item = &Tile> + ExactSizeIterator {
        self.tiles.iter()
    }

    /// Get a mutable iterator over the [`Workspace`]'s [`Tile`]s.
    ///
    /// This includes the fullscreened [`Tile`], if any.
    pub fn tiles_mut(&mut self) -> impl Iterator<Item = &mut Tile> + ExactSizeIterator {
        self.tiles.iter_mut()
    }

    /// Insert a [`Window`] inside this [`Workspace`].
    ///
    /// The workspace will take care of configuring the window's surface for the workspace output.
    pub fn insert_window(&mut self, window: Window, animate: bool) {
        if self.tiles.iter().any(|tile| *tile.window() == window) {
            return;
        }
        self.remove_current_fullscreen();

        window.request_bounds(Some(self.output.geometry().size));
        window.configure_for_output(&self.output);
        let mut tile = Tile::new(window.clone(), Rc::clone(&self.config));
        tile.start_opening_animation();

        let new_idx = if tile.window().fullscreen() {
            // When the window is fullscreened, we insert at the end of the slave stack and set
            // fullscreen_idx. We still dont run the location animation though.
            self.tiles.push(tile);
            let new_idx = self.tiles.len() - 1;
            // Exception is made for fullscreen since its exclusive.
            self.active_tile_idx = Some(new_idx);
            new_idx
        } else {
            match self.config.insert_window_strategy {
                InsertWindowStrategy::EndOfSlaveStack => {
                    self.tiles.push(tile);
                    self.tiles.len() - 1
                }
                InsertWindowStrategy::ReplaceMaster => {
                    self.tiles.insert(0, tile);
                    0
                }
                InsertWindowStrategy::AfterFocused => {
                    let active_idx = self.active_tile_idx.map_or(0, |idx| idx + 1);
                    if active_idx == self.tiles.len() {
                        // Dont wrap around if we are on the last window, to avoid cyclic confusion.
                        self.tiles.push(tile);
                        self.tiles.len() - 1
                    } else {
                        self.tiles.insert(active_idx, tile);
                        active_idx
                    }
                }
            }
        };
        if self.config.focus_new_windows {
            self.active_tile_idx = Some(new_idx)
        }

        self.arrange_tiles(animate);
        self.tiles[new_idx].stop_location_animation();
    }

    /// Remove a [`Window`] from this [`Workspace`].
    ///
    /// This will remove the associated [`Tile`], if you want to run a close animation, see
    /// [`Workspace::close_window`]
    pub fn remove_window(&mut self, window: &Window, animate: bool) -> bool {
        let Some(idx) = self.tiles.iter().position(|tile| tile.window() == window) else {
            return false;
        };

        let window = self.tiles.remove(idx).into_window();
        window.request_bounds(None);
        window.leave_output(&self.output);
        if self.tiles.is_empty() {
            self.active_tile_idx = None;
        } else {
            let idx = self.active_tile_idx.unwrap();
            self.active_tile_idx = Some(idx.clamp(0, self.tiles.len() - 1));
        }

        self.arrange_tiles(animate);

        true
    }

    /// Close the [`Tile`] associated with this [`Window`], running a close animation.
    pub fn close_window(
        &mut self,
        window: &Window,
        renderer: &mut GlowRenderer,
        animate: bool,
    ) -> bool {
        let Some(idx) = self.tiles.iter().position(|tile| tile.window() == window) else {
            return false;
        };
        let _ = self
            .fullscreened_tile_idx
            .take_if(|&mut f_idx| f_idx == idx);

        let tile = self.tiles.remove(idx);
        let scale = self.output.current_scale().fractional_scale().into();
        if animate {
            if let Some(closing_tile) = tile.into_closing_tile(renderer, scale) {
                self.closing_tiles.push(closing_tile);
            }
        }

        self.arrange_tiles(animate);

        true
    }

    /// Prepare the closing animation snapshot for the [`TIle`] associated with this [`Window`].
    ///
    /// We take a capture of the last frame displayed by the window and store it inside a texture
    /// to render it with a [`ClosingTile`]
    pub fn prepare_close_animation_for_window(
        &mut self,
        window: &Window,
        renderer: &mut GlowRenderer,
    ) -> bool {
        let Some(tile) = self.tiles.iter_mut().find(|tile| tile.window() == window) else {
            return false;
        };

        let scale = self.output.current_scale().fractional_scale().into();
        tile.prepare_close_animation_if_needed(renderer, scale);

        true
    }

    /// Clear the taken snapshot for the [`Window`], if any.
    pub fn clear_close_animation_for_window(&mut self, window: &Window) {
        let Some(tile) = self.tiles.iter_mut().find(|tile| tile.window() == window) else {
            return;
        };

        tile.clear_close_animation_snapshot();
    }

    /// Fullscreen the [`Tile`] associated with this window.
    pub fn fullscreen_window(&mut self, window: &Window, animate: bool) -> bool {
        let Some(idx) = self
            .tiles
            .iter_mut()
            .position(|tile| tile.window() == window)
        else {
            return false;
        };
        if Some(idx) == self.fullscreened_tile_idx {
            // We want to fullscreen an already fullscreened window, act as if the request was
            // correctly processed.
            return true;
        }

        self.remove_current_fullscreen();
        self.fullscreened_tile_idx = Some(idx);
        self.arrange_tiles(animate);

        true
    }

    /// Removes the current fullscreened [`Tile`] of this [`Workspace`], if any.
    ///
    /// You must call [`Workspace::arrange_tiles`]
    fn remove_current_fullscreen(&mut self) {
        if let Some(fullscreen_idx) = self.fullscreened_tile_idx.take() {
            self.tiles[fullscreen_idx]
                .window()
                .request_fullscreen(false);
        }
    }

    /// Return whether this [`Workspace`] has a fullscreened [`Tile`].
    pub fn has_fullscreened_tile(&self) -> bool {
        self.fullscreened_tile_idx.is_some()
    }

    /// Get the current fullscreened [`Window`]
    pub fn fullscreened_window(&self) -> Option<Window> {
        self.tiles
            .get(self.fullscreened_tile_idx?)
            .map(Tile::window)
            .cloned()
    }

    /// Get the current fullscreened [`Window`]
    pub fn fullscreened_tile(&self) -> Option<&Tile> {
        self.tiles.get(self.fullscreened_tile_idx?)
    }

    /// Get the location of this [`Window`] relative to this [`Workspace`]
    pub fn window_location(&self, window: &Window) -> Option<Point<i32, Logical>> {
        self.tiles
            .iter()
            .find(|tile| tile.window() == window)
            .map(|tile| tile.location() + tile.window_loc())
    }

    /// Select the next available [`WorkspaceLayout`].
    ///
    /// If the currently selected layout is the last in the layout list, this function cycles back
    /// to the first one.
    pub fn select_next_layout(&mut self, animate: bool) {
        if self.layouts.len() < 2 {
            return;
        }
        self.has_transient_layout_changes = true;

        self.active_layout_idx = match self.active_layout_idx + 1 {
            // When active_layout_idx + 1 == layouts_len, we were on the last element, cycle back.
            idx if idx == self.layouts.len() => 0,
            idx => idx,
        };
        self.arrange_tiles(animate);
    }

    /// Select the previous available [`WorkspaceLayout`].
    ///
    /// If the currently selected layout is the first in the layout list, this function cycles back
    /// to the last one.
    pub fn select_previous_layout(&mut self, animate: bool) {
        if self.layouts.len() < 2 {
            return;
        }
        self.has_transient_layout_changes = true;

        self.active_layout_idx = match self.active_layout_idx.checked_sub(1) {
            // None == overflow occured == we were on the first layout
            None => self.layouts.len() - 1,
            Some(idx) => idx,
        };
        self.arrange_tiles(animate);
    }

    /// Change the master width factor of this [`Workspace`].
    pub fn change_mwfact(&mut self, delta: f64, animate: bool) {
        self.has_transient_layout_changes = true;
        self.mwfact = (self.mwfact + delta).clamp(0.01, 0.99);
        self.arrange_tiles(animate);
    }

    /// Change the number of master windows of this [`Workspace`].
    pub fn change_nmaster(&mut self, delta: i32, animate: bool) {
        self.has_transient_layout_changes = true;
        self.nmaster = self.nmaster.saturating_add_signed(delta as isize).max(1);
        self.arrange_tiles(animate);
    }

    /// Arrange all the [`Tile`]s in this [`Workspace`]
    pub fn arrange_tiles(&mut self, animate: bool) {
        let output_geometry = self.output.geometry();
        if let Some(fullscreen_idx) = self.fullscreened_tile_idx {
            // The fullscreen tile should be positionned at (0,0), the origin of the output.
            self.tiles[fullscreen_idx].set_geometry(
                Rectangle::from_loc_and_size((0, 0), output_geometry.size),
                animate,
            );
        }

        if self.tiles.is_empty() {
            return;
        }

        let (outer_gaps, inner_gaps) = self.gaps;
        let (maximized, tiles) = self
            .tiles
            .iter_mut()
            .partition::<Vec<_>, _>(|tile| tile.window().maximized());
        let work_area = {
            let mut work_area = output_geometry;
            work_area.loc += Point::from((outer_gaps, outer_gaps));
            work_area.size -= Size::from((outer_gaps, outer_gaps)).upscale(2);
            work_area
        };

        for tile in maximized {
            // Maximized tiles get all the work area, while the tiled abide to layout algo.
            tile.set_geometry(work_area, animate);
        }

        let tiles_len = i32::try_from(tiles.len()).expect("tiles.len() overflow");
        let mwfact = self.mwfact;

        // We cant have more nmaster than tiles
        let nmaster = min(
            i32::try_from(self.nmaster).expect("nmaster overflow"),
            tiles_len,
        );
        let mut master_geo @ mut stack_geo = work_area;
        match self.layouts[self.active_layout_idx] {
            WorkspaceLayout::Tile => {
                master_geo.size.h -= (nmaster - 1).max(0) * inner_gaps;
                stack_geo.size.h -= (tiles_len - nmaster - 1).max(0) * inner_gaps;

                if tiles_len > nmaster {
                    stack_geo.size.w =
                        (f64::from(master_geo.size.w - inner_gaps) * (1.0 - mwfact)).round() as i32;
                    master_geo.size.w -= inner_gaps + stack_geo.size.w;
                    stack_geo.loc.x = master_geo.loc.x + master_geo.size.w + inner_gaps;
                };

                let master_heights = {
                    let tiles = tiles.get(0..nmaster as usize).unwrap_or_default();
                    let proportions = tiles
                        .iter()
                        .map(|tile| tile.proportion())
                        .collect::<Vec<_>>();
                    proportion_length(&proportions, master_geo.size.h)
                };

                let stack_heights = {
                    let tiles = tiles.get(nmaster as usize..).unwrap_or_default();
                    let proportions = tiles
                        .iter()
                        .map(|tile| tile.proportion())
                        .collect::<Vec<_>>();
                    proportion_length(&proportions, stack_geo.size.h)
                };

                for (idx, tile) in tiles.into_iter().enumerate() {
                    if (idx as i32) < nmaster {
                        let master_height = master_heights[idx];
                        let geo = Rectangle::from_loc_and_size(
                            master_geo.loc,
                            (master_geo.size.w, master_height),
                        );
                        tile.set_geometry(geo, animate);
                        master_geo.loc.y += master_height + inner_gaps;
                    } else {
                        let stack_height = stack_heights[idx - nmaster as usize];
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
                master_geo.size.w -= (nmaster - 1).max(0) * inner_gaps;
                stack_geo.size.w -= (tiles_len - nmaster).max(0) * inner_gaps;

                if tiles_len > nmaster {
                    stack_geo.size.h =
                        (f64::from(master_geo.size.h - inner_gaps) * (1.0 - mwfact)).round() as i32;
                    master_geo.size.h -= inner_gaps + stack_geo.size.h;
                    stack_geo.loc.y = master_geo.loc.y + master_geo.size.h + inner_gaps;
                };

                let master_widths = {
                    let tiles = tiles.get(0..nmaster as usize).unwrap_or_default();
                    let proportions = tiles
                        .iter()
                        .map(|tile| tile.proportion())
                        .collect::<Vec<_>>();
                    proportion_length(&proportions, master_geo.size.w)
                };

                let stack_widths = {
                    let tiles = tiles.get(nmaster as usize..).unwrap_or_default();
                    let proportions = tiles
                        .iter()
                        .map(|tile| tile.proportion())
                        .collect::<Vec<_>>();
                    proportion_length(&proportions, stack_geo.size.w)
                };

                for (idx, tile) in tiles.into_iter().enumerate() {
                    if (idx as i32) < nmaster {
                        let master_width = master_widths[idx];
                        let geo = Rectangle::from_loc_and_size(
                            master_geo.loc,
                            (master_width, master_geo.size.h),
                        );
                        tile.set_geometry(geo, animate);
                        master_geo.loc.x += master_width + inner_gaps;
                    } else {
                        let stack_width = stack_widths[idx - nmaster as usize];
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

                let mut master_geo @ mut left_geo @ mut right_geo = work_area;
                master_geo.size.h -= inner_gaps * master_len.saturating_sub(1) as i32;
                left_geo.size.h -= inner_gaps * left_len.saturating_sub(1) as i32;
                right_geo.size.h -= inner_gaps * right_len.saturating_sub(1) as i32;

                if tiles_len > nmaster {
                    if (tiles_len - nmaster) > 1 {
                        master_geo.size.w =
                            (f64::from(master_geo.size.w - 2 * inner_gaps) * mwfact).round() as i32;
                        left_geo.size.w =
                            (work_area.size.w - master_geo.size.w - 2 * inner_gaps) / 2;
                        right_geo.size.w =
                            work_area.size.w - master_geo.size.w - 2 * inner_gaps - left_geo.size.w;
                        master_geo.loc.x += left_geo.size.w + inner_gaps;
                    } else {
                        master_geo.size.w =
                            (f64::from(master_geo.size.w - inner_gaps) * mwfact).round() as i32;
                        left_geo.size.w = 0;
                        right_geo.size.w -= master_geo.size.w - inner_gaps;
                    }

                    left_geo.loc = work_area.loc;
                    right_geo.loc = work_area.loc; // for y value only
                    right_geo.loc.x = master_geo.loc.x + master_geo.size.w + inner_gaps;
                }

                let (master_tiles, left_right_tiles) = tiles
                    .into_iter()
                    .enumerate()
                    .partition::<Vec<_>, _>(|(idx, _)| (*idx as i32) < nmaster);
                let (left_tiles, right_tiles) = left_right_tiles
                    .into_iter()
                    .partition::<Vec<_>, _>(|(idx, _)| {
                        ((*idx as i32).saturating_sub(nmaster) % 2) != 0
                    });

                let left_heights = {
                    let proportions = left_tiles
                        .iter()
                        .map(|(_, tile)| tile.proportion())
                        .collect::<Vec<_>>();
                    proportion_length(&proportions, left_geo.size.h)
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
                    let proportions = master_tiles
                        .iter()
                        .map(|(_, tile)| tile.proportion())
                        .collect::<Vec<_>>();
                    proportion_length(&proportions, master_geo.size.h)
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
                    let proportions = right_tiles
                        .iter()
                        .map(|(_, tile)| tile.proportion())
                        .collect::<Vec<_>>();
                    proportion_length(&proportions, right_geo.size.h)
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

    /// Advance animations for this [`Workspace`]
    pub fn advance_animations(&mut self, now: Duration) -> bool {
        self.tiles
            .iter_mut()
            .fold(false, |acc, tile| tile.advance_animations(now) || acc)
            || self.closing_tiles.iter_mut().fold(false, |acc, tile| {
                tile.advance_animations(now);
                acc || tile.is_finished()
            })
    }

    /// Render all the needed elements of this [`Workspace`].
    pub fn render<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        scale: f64,
    ) -> Vec<WorkspaceRenderElement<R>> {
        let mut elements = vec![];

        if let Some(fullscreen_idx) = self.fullscreened_tile_idx {
            // Fullscreen gets rendered above all others.
            //
            // TODO: Maybe fade out the other tiles when fullscreen is resizing? Would be better
            // than just removing them right away.
            let tile = &self.tiles[fullscreen_idx];
            return tile.render(renderer, scale, true).map(Into::into).collect();
        }

        // Render closing tiles above the rest
        for closing_tile in self.closing_tiles.iter() {
            elements.push(closing_tile.render(scale, 1.0).into())
        }

        if let Some(tile) = self.active_tile() {
            // Active gets rendered above others.
            elements.extend(tile.render(renderer, scale, true).map(Into::into));
        }

        // Now render others, just fine.
        for (idx, tile) in self.tiles.iter().enumerate() {
            if Some(idx) == self.active_tile_idx {
                continue; // active tile has already been rendered.
            }

            elements.extend(tile.render(renderer, scale, true).map(Into::into));
        }

        elements
    }
}

fht_render_elements! {
    WorkspaceRenderElement<R> => {
        Tile = TileRenderElement<R>,
        ClosingTile = ClosingTileRenderElement,
    }
}

/// Proportion a given length with given proportions.
///
/// This function ensures that the the returned lengths' sum is equal to `length`
fn proportion_length(proportions: &[f64], length: i32) -> Vec<i32> {
    let total_proportions: f64 = proportions.iter().sum();
    let lengths = proportions
        .iter()
        .map(|&cfact| (length as f64 * (cfact / total_proportions)).floor() as i32)
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
