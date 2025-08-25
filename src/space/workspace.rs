use std::cmp::min;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fht_animation::Animation;
use fht_compositor_config::{InsertWindowStrategy, WorkspaceLayout};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::desktop::layer_map_for_output;
use smithay::output::Output;
use smithay::utils::{IsAlive, Logical, Point, Rectangle, Size};
use smithay::wayland::seat::WaylandFocus;

use super::closing_tile::{ClosingTile, ClosingTileRenderElement};
use super::tile::{Tile, TileRenderElement};
use super::Config;
use crate::fht_render_elements;
use crate::input::resize_tile_grab::ResizeEdge;
use crate::output::OutputExt;
use crate::renderer::FhtRenderer;
use crate::utils::RectCenterExt;
use crate::window::Window;

static WORKSPACE_IDS: AtomicUsize = AtomicUsize::new(0);

/// Identifier of a [`Workspace`].
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct WorkspaceId(pub usize);
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
impl std::ops::Deref for WorkspaceId {
    type Target = usize;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug)]
struct InteractiveResize {
    window: Window,
    initial_window_geometry: Rectangle<i32, Logical>,
    edges: ResizeEdge,
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

    /// Render offset of this workspace.
    ///
    /// This is used to achieve workspace switch animations, this relocates all the generated
    /// render elements from [`Workspace::render`].
    render_offset: Option<Animation<[i32; 2]>>,

    /// Fade out animations for non-fullscreen windows.
    ///
    /// When fullscreening a window, we run a fade-out animations on all other windows in the
    /// workspace to make a seamless transition in and out of fullscreen.
    ///
    /// We keep track of the tile that was fullscreened to avoid fading it out when we remove it
    /// from the fullscreen state.
    ///
    /// If the specified index is [`None`], all [`Tile`]s should fade.
    fullscreen_fade_animation: Option<(Option<usize>, Animation<f32>)>,

    /// An interactive tile resize.
    interactive_resize: Option<InteractiveResize>,

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
            render_offset: None,
            fullscreen_fade_animation: None,
            interactive_resize: None,
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
        crate::profile_function!();
        // Reload the shared Rcs with workspace system config.
        self.config = Rc::clone(config);
        for tile in &mut self.tiles {
            tile.update_config(Rc::clone(config));
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
        crate::profile_function!();
        let mut arrange = false;

        if self
            .fullscreened_tile_idx
            .as_ref()
            .is_some_and(|&idx| self.tiles.get(idx).is_none())
        {
            // Fullscreen tile idx points to non-existent tile!?
            // This should never happen in practice but still handle this edge case.
            let idx = self.fullscreened_tile_idx.take().unwrap();
            self.start_fullscreen_fade_in(Some(idx));
            arrange = true;
        }

        if self
            .fullscreened_tile_idx
            .as_ref()
            .is_some_and(|&fs_idx| Some(fs_idx) != self.active_tile_idx)
        {
            // Two possible cases:
            // - We changed focus while there's a fullscreen tile
            // - The tile order changed.
            // Both of these warrant a layout arrange.
            let idx = self.fullscreened_tile_idx.take().unwrap();
            self.start_fullscreen_fade_in(Some(idx));
            arrange = true;
        }

        if let Some(idx) = self
            .fullscreened_tile_idx
            .take_if(|&mut idx| !self.tiles[idx].window().alive())
        {
            // The previous fullscreen is dead, arrange as a heuristic move
            self.start_fullscreen_fade_in(Some(idx));
            arrange = true;
        }

        if let Some(idx) = self
            .fullscreened_tile_idx
            .take_if(|idx| !self.tiles[*idx].window().fullscreen())
        {
            // The current fullscreened tile window is not fullscreened anymore.
            //
            // This can be caused by user interaction inside the window, for example a unfullscreen
            // button, or a state toggle.
            //
            // This can also be triggered by other parts of the compositor logic, assuming that we
            // (the workspace) will take care of unfullscreening the window.
            self.start_fullscreen_fade_in(Some(idx));
            arrange = true;
        }

        if let Some(fullscreened_tile) = self
            .fullscreened_tile_idx
            .as_ref()
            .map(|&idx| &mut self.tiles[idx])
        {
            fullscreened_tile.refresh(
                &self.output,
                true, // Fullscreen window gets exclusive activation and focus.
            );
        }

        // let _ = self.interactive_swap.take_if(|swap| {
        //     !swap.window.alive() || !self.tiles.iter().any(|tile| *tile.window() == swap.window)
        // });
        let _ = self.interactive_resize.take_if(|swap| {
            !swap.window.alive() || !self.tiles.iter().any(|tile| *tile.window() == swap.window)
        });

        // Clean zombies.
        // Cleaning fullscreen zombie case has been handled above.
        self.tiles.retain(|tile| {
            if !tile.window().alive() {
                arrange = true; // we removed a tile, layout WILL change.
                return false;
            }

            true
        });
        self.closing_tiles.retain(|tile| !tile.is_finished());

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
            tile.refresh(&self.output, Some(idx) == self.active_tile_idx);
        }
    }

    /// Get the [`Workspace`]'s active [`Tile`] index, if any.
    pub fn active_tile_idx(&self) -> Option<usize> {
        self.active_tile_idx
    }

    /// Set the [`Workspace`]'s active [`Tile`] index, if any.
    ///
    /// This will get clamped to a valid value when [`Workspace::refresh`] is called.
    pub fn set_active_tile_idx(&mut self, idx: usize) {
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
    pub fn windows(&self) -> impl ExactSizeIterator<Item = &Window> {
        self.tiles.iter().map(Tile::window)
    }

    /// Get an iterator over the [`Workspace`]'s [`Tile`]s.
    ///
    /// This includes the fullscreened [`Tile`], if any.
    pub fn tiles(&self) -> impl ExactSizeIterator<Item = &Tile> {
        self.tiles.iter()
    }

    /// Get a mutable iterator over the [`Workspace`]'s [`Tile`]s.
    ///
    /// This includes the fullscreened [`Tile`], if any.
    pub fn tiles_mut(&mut self) -> impl ExactSizeIterator<Item = &mut Tile> {
        self.tiles.iter_mut()
    }

    /// Get an iterator over the [`Workspace`]'s [`Tile`]s, in the order they are rendered in.
    ///
    /// The first tile is the topmost one.
    pub fn tiles_in_render_order(&self) -> impl Iterator<Item = &Tile> {
        let fullscreen = self
            .fullscreened_tile_idx
            .and_then(|idx| self.tiles.get(idx))
            .into_iter();

        let (ontop_tiles, normal_tiles) = self
            .tiles
            .iter()
            // The active tile is rendered apart, though
            .filter(|tile| Some(tile.window()) != self.active_tile().map(Tile::window))
            .partition::<Vec<_>, _>(|tile| tile.window().rules().ontop.unwrap_or(false));

        fullscreen
            .chain(ontop_tiles)
            .chain(self.active_tile().into_iter())
            .chain(normal_tiles)
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
        let mut parent_idx = None;
        tile.start_opening_animation();

        if !tile.window().tiled() {
            let rules = tile.window().rules();
            let (centered, centered_in_parent) = (rules.centered, rules.centered_in_parent);
            drop(rules);

            let size = tile.size();
            let output_geometry = self.output.geometry();

            if let Some(true) = centered {
                // Center the window after insertion.
                tile.set_location(
                    output_geometry.center() - size.downscale(2).to_point() - output_geometry.loc,
                    false,
                );
            } else if let Some(true) = centered_in_parent {
                // We must have a parent since this can only be set inside
                // src/handlers/compositor.rs
                let parent_surface = tile.window().toplevel().parent().unwrap();
                parent_idx =
                    if let Some(parent_idx) = self.tiles.iter().position(|tile| {
                        tile.window().wl_surface().as_deref() == Some(&parent_surface)
                    }) {
                        let parent_tile = &self.tiles[parent_idx];
                        let parent_geometry = parent_tile.geometry();

                        let new_location = parent_geometry.center() - size.downscale(2).to_point();
                        if output_geometry.contains_rect(Rectangle::new(new_location, size)) {
                            tile.set_location(new_location, false);
                        } else {
                            // Output geometry cannot contain centered in parent geometry.
                            // Fallback to simple centering
                            tile.set_location(
                                output_geometry.center()
                                    - size.downscale(2).to_point()
                                    - output_geometry.loc,
                                false,
                            );
                        }

                        Some(parent_idx)
                    } else {
                        // We did not find the parent in this workspace.
                        // Fallback to simple centering.
                        tile.set_location(
                            output_geometry.center()
                                - size.downscale(2).to_point()
                                - output_geometry.loc,
                            false,
                        );

                        None
                    };
            }
        }

        let new_idx = if tile.window().fullscreen() {
            // When the window is fullscreened, we insert at the end of the slave stack and set
            // fullscreen_idx. We still dont run the location animation though.
            self.tiles.push(tile);
            let new_idx = self.tiles.len() - 1;
            // Exception is made for fullscreen since its exclusive.
            self.active_tile_idx = Some(new_idx);
            new_idx
        } else if let Some(parent_idx) = parent_idx {
            // If there's a parent index, insert it just after to make a logical stacking order.
            // This is what's done with X11 window managers, to make the parent-child relation
            // obvious.
            //
            // Doing this allows for more natural interactions, opening a child window then closing
            // it automatically focuses the parent window again.
            self.tiles.insert(parent_idx.saturating_sub(1), tile);
            parent_idx
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

    /// Inserts a [`Tile`] inside this workspace.
    ///
    /// - If the tile is floating, it will just be added to the workspace at the same location.
    /// - If the tile is tiled, it will try to insert it at the closest location possible in the
    ///   tile stack, depending on where the pointer position in.
    pub(super) fn insert_tile_with_cursor_position(
        &mut self,
        tile: Tile,
        cursor_position: Point<i32, Logical>,
    ) {
        if self.tiles.is_empty() || !tile.window().tiled() {
            self.tiles.push(tile);
            self.active_tile_idx = Some(self.tiles.len() - 1);
            self.arrange_tiles(true);
            return;
        }

        if self
            .fullscreened_tile_idx
            .and_then(|idx| self.tiles.get(idx))
            .is_some()
        {
            let idx = self.fullscreened_tile_idx.unwrap();
            // If there's a fullscreened tile, just insert it and unfullscreen
            self.remove_current_fullscreen();
            self.tiles.insert(idx, tile);

            return;
        }

        // First we need to first the closest tile.
        let (cursor_x, cursor_y) = cursor_position.into();
        let (closest_idx, closest_tile) = self
            .tiles
            .iter()
            .enumerate()
            .min_by_key(|(_, tile)| {
                let (tile_x, tile_y) = tile.geometry().center().into();
                i32::isqrt((tile_x - cursor_x).pow(2) + (tile_y - cursor_y).pow(2))
            })
            .expect("Should not be empty");
        // Now, based on which quadrant of the closest tile we are in, we determine where to insert
        // the final tile. So you can place the tile between two files, for example.
        let cursor_position_in_tile = (cursor_position - closest_tile.location()).to_f64();
        let size = closest_tile.size().to_f64();
        let mut edges = ResizeEdge::empty();
        if cursor_position_in_tile.x < size.w / 3. {
            edges |= ResizeEdge::LEFT;
        } else if 2. * size.w / 3. < cursor_position_in_tile.x {
            edges |= ResizeEdge::RIGHT;
        }
        if cursor_position_in_tile.y < size.h / 3. {
            edges |= ResizeEdge::TOP;
        } else if 2. * size.h / 3. < cursor_position_in_tile.y {
            edges |= ResizeEdge::BOTTOM;
        }

        // The actual handling depends on the layout.
        match self.current_layout() {
            WorkspaceLayout::Tile => {
                if closest_idx < self.nmaster {
                    if edges.intersects(ResizeEdge::RIGHT) && self.nmaster == self.tiles.len() {
                        // We need a way to create a slave stack when there are only masters window,
                        // this condition covers the following case:
                        //
                        // (the X marks where the cursor could be)
                        //
                        // +--------------------+
                        // |              XXXXXX|
                        // |              XXXXXX|
                        // +--------------------+
                        // +--------------------+
                        // |              XXXXXX|
                        // |              XXXXXX|
                        // +--------------------+
                        //
                        // In this case we want to create a stack stack
                        self.active_tile_idx = Some(self.tiles.len());
                        self.tiles.push(tile);
                    } else if edges.intersects(ResizeEdge::BOTTOM) {
                        // Insert after this master window.
                        self.nmaster += 1;
                        self.active_tile_idx = Some(closest_idx + 1);
                        self.tiles.insert(closest_idx + 1, tile);
                    } else if edges.intersects(ResizeEdge::TOP) {
                        self.nmaster += 1;
                        self.active_tile_idx = Some(closest_idx);
                        self.tiles.insert(closest_idx, tile);
                        // Insert before this master window.
                    } else {
                        // Swap the closest window and the grabbed window.
                        // FIXME: This becomes invalid if the number of windows changed

                        // First insert the grabbed tile.
                        self.active_tile_idx = Some(closest_idx);
                        self.tiles.insert(closest_idx, tile);
                    }
                } else if edges.intersects(ResizeEdge::BOTTOM) {
                    // Insert after this stack window.
                    self.active_tile_idx = Some(closest_idx + 1);
                    self.tiles.insert(closest_idx + 1, tile);
                } else if edges.intersects(ResizeEdge::TOP) {
                    self.active_tile_idx = Some(closest_idx);
                    self.tiles.insert(closest_idx, tile);
                    // Insert before this stack window.
                } else {
                    // Swap the closest window and the grabbed window.
                    // FIXME: This becomes invalid if the number of windows changed

                    // First insert the grabbed tile.
                    self.active_tile_idx = Some(closest_idx);
                    self.tiles.insert(closest_idx, tile);
                }

                self.arrange_tiles(true);
            }
            WorkspaceLayout::BottomStack => {
                if closest_idx < self.nmaster {
                    if edges.intersects(ResizeEdge::BOTTOM) && self.nmaster == self.tiles.len() {
                        // We need a way to create a slave stack when there are only masters window,
                        // this condition covers the following case:
                        //
                        // (the X marks where the cursor could be)
                        //
                        // +---------+----------+
                        // |         |          |
                        // |         |          |
                        // |         |          |
                        // |XXXXXXXXX|XXXXXXXXXX|
                        // |XXXXXXXXX|XXXXXXXXXX|
                        // +---------+----------+
                        //
                        // In this case we want to create a stack
                        self.active_tile_idx = Some(self.tiles.len());
                        self.tiles.push(tile);
                    } else if edges.intersects(ResizeEdge::RIGHT) {
                        // Insert after this master window.
                        self.nmaster += 1;
                        self.active_tile_idx = Some(closest_idx + 1);
                        self.tiles.insert(closest_idx + 1, tile);
                    } else if edges.intersects(ResizeEdge::LEFT) {
                        // Insert before this master window.
                        self.nmaster += 1;
                        self.active_tile_idx = Some(closest_idx);
                        self.tiles.insert(closest_idx, tile);
                    } else {
                        // Swap the closest window and the grabbed window.
                        // FIXME: This becomes invalid if the number of windows changed

                        // First insert the grabbed tile.
                        self.active_tile_idx = Some(closest_idx);
                        self.tiles.insert(closest_idx, tile);
                    }
                } else if edges.intersects(ResizeEdge::RIGHT) {
                    // Insert after this stack window.
                    self.active_tile_idx = Some(closest_idx + 1);
                    self.tiles.insert(closest_idx + 1, tile);
                } else if edges.intersects(ResizeEdge::LEFT) {
                    self.active_tile_idx = Some(closest_idx);
                    self.tiles.insert(closest_idx, tile);
                    // Insert before this stack window.
                } else {
                    // Swap the closest window and the grabbed window.
                    // FIXME: This becomes invalid if the number of windows changed

                    // First insert the grabbed tile.
                    self.active_tile_idx = Some(closest_idx);
                    self.tiles.insert(closest_idx, tile);
                }

                self.arrange_tiles(true);
            }
            WorkspaceLayout::CenteredMaster => {
                if closest_idx < self.nmaster {
                    if edges.intersects(ResizeEdge::RIGHT) && self.nmaster == self.tiles.len() {
                        // We need a way to create a slave stack when there are only masters window,
                        // this condition covers the following case:
                        //
                        // (the X marks where the cursor could be)
                        //
                        // +--------------------+
                        // |              XXXXXX|
                        // |              XXXXXX|
                        // +--------------------+
                        // +--------------------+
                        // |              XXXXXX|
                        // |              XXXXXX|
                        // +--------------------+
                        //
                        // In this case we want to create a stack stack
                        self.active_tile_idx = Some(self.tiles.len());
                        self.tiles.push(tile);
                    } else if edges.intersects(ResizeEdge::BOTTOM) {
                        // Insert after this master window.
                        self.nmaster += 1;
                        self.active_tile_idx = Some(closest_idx + 1);
                        self.tiles.insert(closest_idx + 1, tile);
                    } else if edges.intersects(ResizeEdge::TOP) {
                        self.nmaster += 1;
                        self.active_tile_idx = Some(closest_idx);
                        self.tiles.insert(closest_idx, tile);
                        // Insert before this master window.
                    } else {
                        // Swap the closest window and the grabbed window.
                        // FIXME: This becomes invalid if the number of windows changed

                        // First insert the grabbed tile.
                        self.active_tile_idx = Some(closest_idx);
                        self.tiles.insert(closest_idx, tile);
                    }
                } else {
                    // Centered master layout is way too confusing to get something that works right
                    // with the dynamic layout system. Sooo, I am just not bothering for the slave
                    // stack.
                    self.active_tile_idx = Some(closest_idx);
                    self.tiles.insert(closest_idx, tile);
                }

                self.arrange_tiles(true);
            }
            WorkspaceLayout::Floating => {
                // Just insert it, who cares really.
                self.tiles.push(tile);
            }
        }

        self.arrange_tiles(true);
    }

    /// Remove a [`Window`] from this [`Workspace`].
    ///
    /// This will remove the associated [`Tile`], if you want to run a close animation, see
    /// [`Workspace::close_window`]
    pub fn remove_window(&mut self, window: &Window, animate: bool) -> bool {
        let Some(idx) = self.tiles.iter().position(|tile| tile.window() == window) else {
            return false;
        };
        if self
            .fullscreened_tile_idx
            .take_if(|&mut fs_idx| fs_idx == idx)
            .is_some()
        {
            // if we remomved the fullscreen tile, we run the animation ourselves.
            if animate {
                self.start_fullscreen_fade_in(None);
            }
        } else {
            // Otherwise, use remove_current_fullscreen (removed something else)
            self.remove_current_fullscreen();
        }

        let window = self.tiles.remove(idx).into_window();
        window.request_bounds(None);
        window.leave_output(&self.output);
        if self.tiles.is_empty() {
            self.active_tile_idx = None;
        } else {
            let idx = self.active_tile_idx.unwrap();
            self.active_tile_idx = Some(idx.clamp(0, self.tiles.len() - 1));
        }

        self.refresh();
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

        if self.tiles.is_empty() {
            self.active_tile_idx = None;
        } else {
            let idx = self.active_tile_idx.unwrap();
            self.active_tile_idx = Some(idx.clamp(0, self.tiles.len() - 1));
        }

        self.refresh();
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

        let scale = self.output.current_scale().integer_scale();
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
        if animate {
            self.start_fullscreen_fade_out(idx);
        }

        true
    }

    /// Removes the current fullscreened [`Tile`] of this [`Workspace`], if any.
    ///
    /// You must call [`Workspace::arrange_tiles`]
    fn remove_current_fullscreen(&mut self) {
        if let Some(fullscreen_idx) = self.fullscreened_tile_idx.take() {
            self.start_fullscreen_fade_in(Some(fullscreen_idx));
            self.tiles[fullscreen_idx]
                .window()
                .request_fullscreen(false);
        }
    }

    /// Get the current fullscreened [`Tile`] index.
    pub fn fullscreened_tile_idx(&self) -> Option<usize> {
        self.fullscreened_tile_idx
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

    /// The master width factor of this [`Workspace`].
    pub fn mwfact(&self) -> f64 {
        self.mwfact
    }

    /// Change the master width factor of this [`Workspace`].
    pub fn change_mwfact(&mut self, delta: f64, animate: bool) {
        self.has_transient_layout_changes = true;
        self.mwfact = (self.mwfact + delta).clamp(0.01, 0.99);
        self.arrange_tiles(animate);
    }

    /// Set the master width factor of this [`Workspace`].
    pub fn set_mwfact(&mut self, value: f64, animate: bool) {
        self.has_transient_layout_changes = true;
        self.mwfact = value.clamp(0.01, 0.99);
        self.arrange_tiles(animate);
    }

    /// The number of master windows factor of this [`Workspace`].
    pub fn nmaster(&self) -> usize {
        self.nmaster
    }

    /// Change the number of master windows of this [`Workspace`].
    pub fn change_nmaster(&mut self, delta: i32, animate: bool) {
        self.has_transient_layout_changes = true;
        self.nmaster = self.nmaster.saturating_add_signed(delta as isize).max(1);
        self.arrange_tiles(animate);
    }

    /// Set the number of master windows of this [`Workspace`].
    pub fn set_nmaster(&mut self, value: usize, animate: bool) {
        self.has_transient_layout_changes = true;
        self.nmaster = value.max(1);
        self.arrange_tiles(animate);
    }

    /// Prepare an unconfigured [`Window`] for insertion in the workspace.
    ///
    /// This function runs the same algorithms that are used inside [`Self::arrange_tiles`] but
    /// without affecting the already inserted tiles inside the workspace.
    pub fn prepare_unconfigured_window(&self, unconfigured_window: &Window) {
        crate::profile_function!();
        let mut output_geometry = self.output.geometry();
        output_geometry.loc = Point::default(); // tile locations are all relative to output

        if !unconfigured_window.tiled() {
            // The window is floating, no need to send a size at all
            return;
        }

        let rules = unconfigured_window.rules();
        let border_width = self.config.border.with_overrides(&rules.border).thickness;
        let prepared_proportion = rules.proportion.unwrap_or(1.0);

        if unconfigured_window.fullscreen() {
            let fullscreen_size = Size::<_, Logical>::from((
                output_geometry.size.w - (2 * border_width),
                output_geometry.size.h - (2 * border_width),
            ));

            unconfigured_window.request_size(fullscreen_size);
            return;
        }

        let (outer_gaps, inner_gaps) = self.gaps;
        let work_area = calculate_work_area(&self.output, outer_gaps);

        if self.tiles.is_empty() || unconfigured_window.maximized() {
            let maximized_size = Size::<_, Logical>::from((
                work_area.size.w - (2 * border_width),
                work_area.size.h - (2 * border_width),
            ));

            unconfigured_window.request_size(maximized_size);
            return;
        }

        // Now we only care about the tiled windows.
        //
        // The tile structs are just annoying to deal with, since in order to "insert" the to-be
        // -prepared tile inside, we need to instantiate a new Tile. Instead just work with
        // [f64] (where the f64 is the proportion)
        let active_tile = self
            .active_tile()
            .expect("there should be an active tile here");
        let mut active_idx = None;
        let mut tiled_proportions: Vec<_> = self
            .tiles
            .iter()
            .filter(|tile| tile.window().tiled() && !tile.window().maximized())
            .enumerate()
            .map(|(idx, tile)| {
                if tile.window() == active_tile.window() {
                    active_idx = Some(idx);
                }
                tile.proportion()
            })
            .collect();

        let unconfigured_idx = match self.config.insert_window_strategy {
            InsertWindowStrategy::EndOfSlaveStack => {
                tiled_proportions.push(prepared_proportion);
                tiled_proportions.len() - 1
            }
            InsertWindowStrategy::ReplaceMaster => {
                tiled_proportions.insert(0, prepared_proportion);
                0
            }
            InsertWindowStrategy::AfterFocused => {
                let active_idx = active_idx.map_or(0, |idx| idx + 1);
                if active_idx == tiled_proportions.len() {
                    // Dont wrap around if we are on the last window, to avoid cyclic confusion.
                    tiled_proportions.push(prepared_proportion);
                    tiled_proportions.len() - 1
                } else {
                    tiled_proportions.insert(active_idx, prepared_proportion);
                    active_idx
                }
            }
        };

        let tiles_len =
            i32::try_from(tiled_proportions.len()).expect("tiled_windows.len() overflow");
        let mwfact = self.mwfact;
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

                if (0..nmaster).contains(&(unconfigured_idx as i32)) {
                    let tiles = tiled_proportions
                        .get(0..nmaster as usize)
                        .unwrap_or_default();
                    let proportions = tiles.to_vec();
                    let lengths = proportion_length(&proportions, master_geo.size.h);
                    // subtract border, of course.
                    let prepared_height = lengths[unconfigured_idx] - (2 * border_width);
                    let prepared_width = master_geo.size.w - (2 * border_width);
                    unconfigured_window.request_size(Size::from((prepared_width, prepared_height)));
                } else {
                    let tiles = tiled_proportions
                        .get(nmaster as usize..)
                        .unwrap_or_default();
                    let proportions = tiles.to_vec();
                    let lengths = proportion_length(&proportions, stack_geo.size.h);
                    // subtract border, of course.
                    let prepared_height =
                        lengths[unconfigured_idx - nmaster as usize] - (2 * border_width);
                    let prepared_width = master_geo.size.w - (2 * border_width);
                    unconfigured_window.request_size(Size::from((prepared_width, prepared_height)));
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

                if (0..nmaster).contains(&(unconfigured_idx as i32)) {
                    let tiles = tiled_proportions
                        .get(0..nmaster as usize)
                        .unwrap_or_default();
                    let proportions = tiles.to_vec();
                    let lengths = proportion_length(&proportions, master_geo.size.w);
                    // subtract border, of course.
                    let prepared_width = lengths[unconfigured_idx] - (2 * border_width);
                    let prepared_height = master_geo.size.h - (2 * border_width);
                    unconfigured_window.request_size(Size::from((prepared_width, prepared_height)));
                } else {
                    let tiles = tiled_proportions
                        .get(nmaster as usize..)
                        .unwrap_or_default();
                    let proportions = tiles.to_vec();
                    let lengths = proportion_length(&proportions, stack_geo.size.w);
                    // subtract border, of course.
                    let prepared_width =
                        lengths[unconfigured_idx - nmaster as usize] - (2 * border_width);
                    let prepared_height = master_geo.size.w - (2 * border_width);
                    unconfigured_window.request_size(Size::from((prepared_width, prepared_height)));
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

                // Due to how the CenteredMaster layout works, we keep around the original index
                // to find the unconfigured_window back, this forces us to use looks
                let (master_proportions, left_right_proportions) = tiled_proportions
                    .into_iter()
                    .enumerate()
                    // .map(|(original_idx, prop)| (original_idx, prop))
                    .partition::<Vec<_>, _>(|(idx, _)| (*idx as i32) < nmaster);
                let (left_proportions, right_proportions) = left_right_proportions
                    .into_iter()
                    .partition::<Vec<_>, _>(|(idx, _)| {
                        ((*idx as i32).saturating_sub(nmaster) % 2) != 0
                    });

                if unconfigured_idx < nmaster as usize {
                    let master_heights = {
                        let proportions = master_proportions
                            .iter()
                            .map(|(_, prop)| *prop)
                            .collect::<Vec<_>>();
                        let heights = proportion_length(&proportions, master_geo.size.h);
                        heights
                            .into_iter()
                            .zip(master_proportions)
                            .map(|(height, (idx, _))| (idx, height))
                    };
                    for (idx, height) in master_heights {
                        if idx == unconfigured_idx {
                            let size = Size::from((master_geo.size.w - 2 * border_width, height));
                            unconfigured_window.request_size(size);
                            return;
                        }
                    }
                } else if unconfigured_idx % 2 == 0 {
                    // With how CenteredMaster logic works, pair indexes are on the right col.
                    let right_heights = {
                        let proportions = right_proportions
                            .iter()
                            .map(|(_, prop)| *prop)
                            .collect::<Vec<_>>();
                        let heights = proportion_length(&proportions, right_geo.size.h);
                        heights
                            .into_iter()
                            .zip(right_proportions)
                            .map(|(height, (idx, _))| (idx, height))
                    };
                    for (idx, height) in right_heights {
                        if idx == unconfigured_idx {
                            let size = Size::from((right_geo.size.w - 2 * border_width, height));
                            unconfigured_window.request_size(size);
                            return;
                        }
                    }
                } else {
                    let left_heights = {
                        let proportions = left_proportions
                            .iter()
                            .map(|(_, prop)| *prop)
                            .collect::<Vec<_>>();
                        let heights = proportion_length(&proportions, left_geo.size.h);
                        heights
                            .into_iter()
                            .zip(left_proportions)
                            .map(|(height, (idx, _))| (idx, height))
                    };
                    for (idx, height) in left_heights {
                        if idx == unconfigured_idx {
                            let size = Size::from((left_geo.size.w - 2 * border_width, height));
                            unconfigured_window.request_size(size);
                            return;
                        }
                    }
                }
            }
            WorkspaceLayout::Floating => {}
        }
    }

    /// Get the current [`WorkspaceLayout` this [`Workspace`] is using.
    pub fn current_layout(&self) -> WorkspaceLayout {
        self.layouts[self.active_layout_idx]
    }

    /// Arrange all the [`Tile`]s in this [`Workspace`]
    pub fn arrange_tiles(&mut self, animate: bool) {
        crate::profile_function!();
        let mut output_geometry = self.output.geometry();
        output_geometry.loc = Point::default(); // tile locations are all relative to output

        if let Some(fullscreen_idx) = self.fullscreened_tile_idx {
            // The fullscreen tile should be positionned at (0,0), the origin of the output.
            self.tiles[fullscreen_idx].set_geometry(output_geometry, animate);
        }

        if self.tiles.is_empty() {
            return;
        }

        let (outer_gaps, inner_gaps) = self.gaps;

        // We distinguish between tiled, maximized, and floating since a floating tile can be
        // maximized.
        let mut maximized_tiles = vec![];
        let mut tiled = vec![];

        for tile in self.tiles.iter_mut() {
            let window = tile.window();
            match (window.tiled(), window.maximized()) {
                (true, false) => tiled.push(tile),
                // Maximized gets maximized regardless of floating status.
                (_, true) => maximized_tiles.push(tile),
                // Otherwise we don't touch floating tiles
                _ => (),
            }
        }

        let layout = self.current_layout();
        let (maximized, tiles) = self
            .tiles
            .iter_mut()
            // We do not want to affect the fullscreened tile
            .filter(|tile| tile.window().tiled() && !tile.window().fullscreen())
            .partition::<Vec<_>, _>(|tile| tile.window().maximized());
        let work_area = calculate_work_area(&self.output, outer_gaps);

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
        match layout {
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
                    if Some(idx) == self.fullscreened_tile_idx {
                        // Don't affect the fullscreened tile.
                        //
                        // This code does have a side effect of leaving a "hole" inside the layout,
                        // where the fullscreened tile was previously. it's fine, especailly when
                        // it will be paired with a fade-out animation for other tiles.
                        continue;
                    }
                    if (idx as i32) < nmaster {
                        let master_height = master_heights[idx];
                        let geo = Rectangle::new(
                            master_geo.loc,
                            (master_geo.size.w, master_height).into(),
                        );
                        tile.set_geometry(geo, animate);
                        master_geo.loc.y += master_height + inner_gaps;
                    } else {
                        let stack_height = stack_heights[idx - nmaster as usize];
                        let new_geo =
                            Rectangle::new(stack_geo.loc, (stack_geo.size.w, stack_height).into());
                        tile.set_geometry(new_geo, animate);
                        stack_geo.loc.y += stack_height + inner_gaps;
                    }
                }
            }
            WorkspaceLayout::BottomStack => {
                master_geo.size.w -= (nmaster - 1).max(0) * inner_gaps;
                stack_geo.size.w -= (tiles_len - nmaster - 1).max(0) * inner_gaps;

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
                        let geo = Rectangle::new(
                            master_geo.loc,
                            (master_width, master_geo.size.h).into(),
                        );
                        tile.set_geometry(geo, animate);
                        master_geo.loc.x += master_width + inner_gaps;
                    } else {
                        let stack_width = stack_widths[idx - nmaster as usize];
                        let geo =
                            Rectangle::new(stack_geo.loc, (stack_width, stack_geo.size.h).into());
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
                    let geo = Rectangle::new(left_geo.loc, (left_geo.size.w, height).into());
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
                    let geo = Rectangle::new(master_geo.loc, (master_geo.size.w, height).into());
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
                    let geo = Rectangle::new(right_geo.loc, (right_geo.size.w, height).into());
                    tile.set_geometry(geo, animate);
                    right_geo.loc.y += height + inner_gaps;
                }
            }
            WorkspaceLayout::Floating => {}
        }
    }

    /// Start an interactive swap grab.
    ///
    /// Returns [`true`] if the grab was successfully registered.
    pub(super) fn start_interactive_swap(&mut self, window: &Window) -> Option<Tile> {
        let Some(idx) = self.tiles.iter().position(|tile| tile.window() == window) else {
            // Can't find the adequate tile
            return None;
        };

        if window.fullscreen() {
            // Fullscreening should be exclusive.
            assert_eq!(self.fullscreened_window().as_ref(), Some(window));
            window.request_fullscreen(false);
            self.fullscreened_tile_idx = None;
            // Start fading in windows as we grab the fullscreened tile
            self.start_fullscreen_fade_in(None);
        }

        // Reset window state
        window.request_fullscreen(false);
        window.request_maximized(false);

        let tile = self.tiles.remove(idx);
        if idx < self.nmaster {
            self.nmaster = (self.nmaster - 1).max(1);
        }
        self.arrange_tiles(true);
        Some(tile)
    }

    /// Start an interactive resize grab.
    ///
    /// Returns [`true`] if the grab was successfully registered.
    pub fn start_interactive_resize(&mut self, window: &Window, edges: ResizeEdge) -> bool {
        if self.interactive_resize.is_some() {
            // Can't have two interactive resizes at a time.
            return false;
        }

        let Some(tile) = self.tiles.iter().find(|tile| tile.window() == window) else {
            // Can't find the adequate tile
            return false;
        };

        match (window.tiled(), self.current_layout()) {
            (_, WorkspaceLayout::Floating) | (false, _) => (),
            // We only do interactive resizes on floating windows
            (true, _) => return false,
        }

        let loc = tile.visual_location();
        let size = window.size();
        self.interactive_resize = Some(InteractiveResize {
            window: window.clone(),
            initial_window_geometry: Rectangle::new(loc, size),
            edges,
        });

        true
    }

    /// Handle an interactive resize grab motion.
    ///
    /// `delta` is how much the cursor moved from its initial window location.
    ///
    /// Returns [`true`] if the grab was successfully registered.
    pub fn handle_interactive_resize_motion(
        &mut self,
        window: &Window,
        delta: Point<i32, Logical>,
    ) -> bool {
        let Some(interactive_resize) = &self.interactive_resize else {
            return false;
        };

        if interactive_resize.window != *window {
            return false;
        }

        let active_window = self.active_window();
        if Some(window) != active_window.as_ref() {
            return false;
        }

        match (window.tiled(), self.current_layout()) {
            (_, WorkspaceLayout::Floating) | (false, _) => (),
            // We switched from floating to tiled between the motion events
            // Can happen if the user uses a key action bound to toggle-window-floating
            (true, _) => return false,
        }

        let mut new_size = interactive_resize.initial_window_geometry.size;
        let (mut dx, mut dy) = (delta.x, delta.y);
        if interactive_resize.edges.intersects(ResizeEdge::LEFT) {
            // If we are grabbing from the left edge, we are expanding the window from the left.
            // Due to how the coordinate system works, we inverse the delta to achieve this.
            dx = -dx;
        }
        if interactive_resize.edges.intersects(ResizeEdge::TOP) {
            // Same deal if we are gradding from the top.
            dy = -dy;
        }

        if interactive_resize
            .edges
            .intersects(ResizeEdge::LEFT | ResizeEdge::RIGHT)
        {
            new_size.w += dx;
        }

        if interactive_resize
            .edges
            .intersects(ResizeEdge::TOP | ResizeEdge::BOTTOM)
        {
            new_size.h += dy;
        }

        window.request_size(new_size);

        true
    }

    /// Handle an interactive resize grab motion.
    ///
    /// `position` is the cursor position relative to the workspace.
    ///
    /// Returns [`true`] if the grab was successfully registered.
    pub fn handle_interactive_resize_end(
        &mut self,
        window: &Window,
        _: Point<f64, Logical>,
    ) -> bool {
        let Some(interactive_resize) = self.interactive_resize.take() else {
            return false;
        };

        if interactive_resize.window != *window {
            return false;
        }

        true
    }

    /// Returns the current value of the render offset
    pub fn render_offset(&self) -> Option<Point<i32, Logical>> {
        self.render_offset
            .as_ref()
            .map(Animation::value)
            .map(|&[x, y]| Point::from((x, y)))
    }

    /// Start a render offset animation
    pub fn start_render_offset_animation(
        &mut self,
        mut start: Point<i32, Logical>,
        end: Point<i32, Logical>,
        animation_config: &super::AnimationConfig,
    ) {
        if let Some(animation) = self.render_offset.take() {
            let [x, y] = *animation.value();
            start = Point::from((x, y));
        }

        self.render_offset = Some(
            Animation::new(
                [start.x, start.y],
                [end.x, end.y],
                animation_config.duration,
            )
            .with_curve(animation_config.curve),
        );
    }

    /// Advance animations for this [`Workspace`]
    pub fn advance_animations(&mut self, target_presentation_time: Duration) -> bool {
        crate::profile_function!();
        let mut running = false;

        let _ = self.render_offset.take_if(|a| a.is_finished());
        if let Some(animation) = &mut self.render_offset {
            animation.tick(target_presentation_time);
            running = true;
        }

        let _ = self
            .fullscreen_fade_animation
            .take_if(|(_, a)| a.is_finished());
        if let Some((_, animation)) = &mut self.fullscreen_fade_animation {
            animation.tick(target_presentation_time);
            running = true;
        }

        for tile in &mut self.tiles {
            running |= tile.advance_animations(target_presentation_time);
        }

        for closing_tile in &mut self.closing_tiles {
            closing_tile.advance_animations(target_presentation_time);
            running = true;
        }

        running
    }

    /// Start the fullscreen fade out animation.
    fn start_fullscreen_fade_out(&mut self, idx: usize) {
        if let Some(animation_config) = self.config.window_geometry_animation.as_ref() {
            let duration = animation_config.duration / 2;
            let start = self
                .fullscreen_fade_animation
                .take()
                .map(|(_, anim)| *anim.value())
                .unwrap_or(1.0);
            self.fullscreen_fade_animation = Some((
                Some(idx),
                Animation::new(start, 0.0, duration).with_curve(animation_config.curve),
            ));
        }
    }

    /// Start the fullscreen fade in animation.
    fn start_fullscreen_fade_in(&mut self, idx: Option<usize>) {
        if let Some(animation_config) = self.config.window_geometry_animation.as_ref() {
            let duration = animation_config.duration / 2;
            let start = self
                .fullscreen_fade_animation
                .take()
                .map(|(_, anim)| *anim.value())
                .unwrap_or(0.0);
            self.fullscreen_fade_animation = Some((
                idx,
                Animation::new(start, 1., duration).with_curve(animation_config.curve),
            ));
        }
    }

    /// Render all the needed elements of this [`Workspace`].
    ///
    /// If `render_offset` is `Some`, the workspace will use this value instead of the one it's
    /// currently animated with. The current purpose of this is just for screencasting purposes.
    pub fn render<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        scale: i32,
        render_offset: Option<Point<i32, Logical>>,
    ) -> Vec<WorkspaceRenderElement<R>> {
        crate::profile_function!();
        let mut elements = vec![];
        // when fullscreening a window, we apply a decreasing alpha to other tiles in order to make
        // the transition seamless when entering/closing fullscreen
        let (skip_alpha_animation_idx, alpha) = self
            .fullscreen_fade_animation
            .as_ref()
            .map(|(idx, anim)| (*idx, *anim.value()))
            .unwrap_or((None, 1.0));

        let render_offset = render_offset
            .or_else(|| self.render_offset())
            .unwrap_or_default();

        if let Some(fullscreen_idx) = self.fullscreened_tile_idx {
            // Fullscreen gets rendered above all others.
            let tile = &self.tiles[fullscreen_idx];

            let fullscreen_elements = tile
                .render(renderer, scale, 1.0, &self.output, render_offset)
                .map(Into::into);

            if skip_alpha_animation_idx.is_none() {
                return fullscreen_elements.collect();
            }
        }

        // Render closing tiles above the rest
        for closing_tile in self.closing_tiles.iter() {
            let element = closing_tile.render(scale, alpha).into();
            elements.push(element);
        }

        let (ontop_tiles, normal_tiles) = self
            .tiles
            .iter()
            .enumerate()
            // Render the active tile apart, above the rest, but below the closing tiles
            .filter(|(idx, _)| Some(*idx) != self.active_tile_idx)
            .partition::<Vec<_>, _>(|(_, tile)| tile.window().rules().ontop.unwrap_or(false));

        let mut render_tile = |idx, tile: &Tile| {
            let alpha = if Some(idx) == skip_alpha_animation_idx {
                1.0
            } else {
                alpha
            };

            elements.extend(
                tile.render(renderer, scale, alpha, &self.output, render_offset)
                    .map(Into::into),
            );
        };

        // First render ontop tiles.
        for (idx, tile) in ontop_tiles.into_iter() {
            render_tile(idx, tile)
        }

        // Then the active one
        if let Some(active_tile) = self.active_tile() {
            render_tile(self.active_tile_idx.unwrap(), active_tile)
        }

        // Now render others, just fine.
        for (idx, tile) in normal_tiles.into_iter() {
            render_tile(idx, tile)
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

fn calculate_work_area(output: &Output, outer_gaps: i32) -> Rectangle<i32, Logical> {
    let mut work_area = layer_map_for_output(output).non_exclusive_zone();
    work_area.loc += Point::from((outer_gaps, outer_gaps));
    work_area.size -= Size::from((outer_gaps, outer_gaps)).upscale(2);
    work_area
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
