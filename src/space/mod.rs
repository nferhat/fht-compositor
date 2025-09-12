//! Space logic for `fht-compositor`
//!
//! The compositor follows a static workspace model, each output gets associated a [`Monitor`] that
//! identifies it inside the compositor layout logic. Each layout contains a defined number of
//! [`Workspace`](workspace::Workspace)s that [`Window`]s get mapped in.
//!
//! Each workspace partitions their windows accordingly inside the output space using a layout that
//! defines an algorithm that separates windows in a tree of windows. When adding a new window, the
//! workspace can either insert it in the current node windows (that are tiled in a direction) or
//! can create a new node thats opposite to the current node direction (either vertical or
//! horizontal)

use std::rc::Rc;
use std::time::Duration;

use fht_animation::AnimationCurve;
pub use monitor::{Monitor, MonitorRenderElement, MonitorRenderResult};
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::desktop::WindowSurfaceType;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::seat::WaylandFocus;
pub use tile::TileRenderElement;
#[allow(unused)] // re-export WorkspaceRenderElement for screencopy type bounds
pub use workspace::{Workspace, WorkspaceId, WorkspaceRenderElement};

use crate::input::resize_tile_grab::ResizeEdge;
use crate::output::OutputExt;
use crate::renderer::FhtRenderer;
use crate::utils::RectCenterExt;
use crate::window::Window;

mod border;
mod tree;
mod closing_tile;
mod monitor;
pub mod shadow;
mod tile;
mod workspace;

/// The workspace system [`Space`].
pub struct Space {
    /// The [`Monitor`]s tracked by the [`Space`]
    monitors: Vec<Monitor>,

    /// The index of the primary [`Monitor`].
    ///
    /// Usually this is the first added [`Monitor`]. In case the primary [`Monitor`] gets removed,
    /// this index is incremented by one.
    primary_idx: usize,

    /// The index of the active [`Monitor`].
    ///
    /// This should be the monitor that has the pointer cursor in its bounds.
    active_idx: usize,

    /// Interactive move/swap state.
    ///
    /// It has to live in the space to handle cross-monitor tile moving.
    interactive_swap: Option<InteractiveSwap>,

    /// Shared configuration with across the workspace system.
    config: Rc<Config>,
}

struct InteractiveSwap {
    /// The tile currently being swapped around.
    tile: tile::Tile,
    /// We need to track on which outputs the tile is visible on, to render it accordingly.
    overlap_outputs: Vec<Output>,
}

impl Space {
    /// Create a new [`Space`].
    pub fn new(config: &fht_compositor_config::Config) -> Self {
        let config = Config::new(config).expect("Space configuration invariants!");
        Self {
            monitors: vec![],
            primary_idx: 0,
            active_idx: 0,
            interactive_swap: None,
            config: Rc::new(config),
        }
    }

    /// Run periodic clean-up tasks.
    pub fn refresh(&mut self) {
        crate::profile_function!();

        if let Some(InteractiveSwap {
            tile,
            overlap_outputs,
            ..
        }) = &self.interactive_swap
        {
            tile.window().request_activated(true);
            let bbox = tile.window().bbox();

            for output in overlap_outputs {
                let output_geometry = output.geometry();
                let mut bbox = bbox;
                bbox.loc = tile.location() + tile.window_loc() + output_geometry.loc;
                if let Some(mut overlap) = output_geometry.intersection(bbox) {
                    // overlap must be in window-local coordinates.
                    overlap.loc -= bbox.loc;
                    tile.window().enter_output(output, overlap);
                }
            }

            tile.window().send_pending_configure();
            tile.window().refresh();
        }

        for (idx, monitor) in self.monitors.iter_mut().enumerate() {
            monitor.refresh(idx == self.active_idx)
        }
    }

    /// Advance the animations for the [`Monitor`] associated with this [`Output`].
    pub fn advance_animations(
        &mut self,
        target_presentation_time: Duration,
        output: &Output,
    ) -> bool {
        let mut ongoing = false;

        if let Some(InteractiveSwap {
            tile,
            overlap_outputs,
            ..
        }) = &mut self.interactive_swap
        {
            if overlap_outputs.contains(output) && tile.advance_animations(target_presentation_time)
            {
                ongoing = true;
            }
        }

        let Some(monitor) = self.monitors.iter_mut().find(|mon| mon.output() == output) else {
            warn!("Called Space::advance_animations with invalid output");
            return ongoing;
        };

        monitor.advance_animations(target_presentation_time) || ongoing
    }

    /// Reload the [`Config`] of the [`Space`].
    pub fn reload_config(&mut self, config: &fht_compositor_config::Config) {
        crate::profile_function!();
        let config = Config::new(config).expect("Space configuration invariants");
        self.config = Rc::new(config);

        for monitor in &mut self.monitors {
            monitor.config = Rc::clone(&self.config);
            for workspace in monitor.workspaces_mut() {
                workspace.reload_config(&self.config)
            }
        }
    }

    /// Get an iterator over the [`Space`]'s tracked [`Monitor`](s)
    pub fn monitors(&self) -> impl ExactSizeIterator<Item = &Monitor> {
        self.monitors.iter()
    }

    /// Get a mutable iterator over the [`Space`]'s tracked [`Monitor`](s)
    pub fn monitors_mut(&mut self) -> impl ExactSizeIterator<Item = &mut Monitor> {
        self.monitors.iter_mut()
    }

    /// Get the [`Monitor`] associated with this [`Output`].
    pub fn monitor_for_output(&self, output: &Output) -> Option<&Monitor> {
        self.monitors.iter().find(|mon| mon.output() == output)
    }

    /// Get the [`Monitor`] associated with this [`Output`].
    pub fn monitor_mut_for_output(&mut self, output: &Output) -> Option<&mut Monitor> {
        self.monitors.iter_mut().find(|mon| mon.output() == output)
    }

    /// Get the visible [`Window`]s for the associated [`Output`].
    pub fn visible_windows_for_output(&self, output: &Output) -> impl Iterator<Item = &Window> {
        let monitor_windows = self
            .monitors
            .iter()
            .find(|mon| mon.output() == output)
            .into_iter()
            .flat_map(|mon| mon.visible_windows());
        let interactive_swap_window = self
            .interactive_swap
            .as_ref()
            .filter(|swap| swap.overlap_outputs.contains(output))
            .map(|swap| swap.tile.window());

        interactive_swap_window.into_iter().chain(monitor_windows)
    }

    /// Get the [`Window`]s on the associated [`Output`].
    #[allow(unused)]
    pub fn windows_on_output(&self, output: &Output) -> impl Iterator<Item = &Window> {
        self.monitors
            .iter()
            .find(|mon| mon.output() == output)
            .into_iter()
            .flat_map(Monitor::workspaces)
            .flat_map(Workspace::windows)
    }

    /// Get an iterator of all the [`Output`]s managed by this [`Space`].
    pub fn outputs(&self) -> impl ExactSizeIterator<Item = &Output> {
        self.monitors.iter().map(Monitor::output)
    }

    /// Get an iterator of all the [`Windows`]s managed by this [`Space`].
    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.monitors
            .iter()
            .flat_map(Monitor::workspaces)
            .flat_map(Workspace::windows)
    }

    /// Get a mutable iterator of all the [`Tile`]s managed by this [`Space`].
    pub fn tiles_mut(&mut self) -> impl Iterator<Item = &mut tile::Tile> {
        self.monitors
            .iter_mut()
            .flat_map(Monitor::workspaces_mut)
            .flat_map(Workspace::tiles_mut)
    }

    /// Add an [`Output`] to this [`Space`].
    pub fn add_output(&mut self, output: Output) {
        let monitor = Monitor::new(output, Rc::clone(&self.config));
        self.monitors.push(monitor);
    }

    /// Removes an [`Output`] to this [`Space`].
    ///
    /// # Current behaviour
    ///
    /// When removing an output, its corresponding [`Monitor`] will be removed from the [`Space`].
    /// Its [`Workspace`]s will get merged with the primary [`Monitor`]'s workspaces.
    pub fn remove_output(&mut self, output: &Output) {
        let Some(removed_idx) = self.monitors.iter().position(|mon| mon.output() == output) else {
            warn!(
                output = output.name(),
                "Tried to remove an output that wasn't tracked by the Space"
            );
            return;
        };

        let removed = self.monitors.remove(removed_idx);
        if self.monitors.is_empty() {
            self.primary_idx = 0;
            self.active_idx = 0;
            // When we don't have any monitors left, the compositor exits out
            return;
        }

        // We want to keep both the primary monitor and active monitor indexes updated.
        // Vec::remove shifts all the elements after the index, adapt accordingly.
        if self.primary_idx >= removed_idx {
            self.primary_idx = self.primary_idx.saturating_sub(1);
        }
        if self.active_idx >= removed_idx {
            self.active_idx = self.active_idx.saturating_sub(1);
        }

        self.monitors[self.primary_idx].merge_with(removed);
    }

    /// Arrange the [`Monitor`] of this [`Output`].
    ///
    /// You should call this when [`Output`]'s geometry changes.
    pub fn output_resized(&mut self, output: &Output, animate: bool) {
        crate::profile_function!();
        let Some(monitor) = self.monitors.iter_mut().find(|mon| mon.output() == output) else {
            warn!("Tried to call output_resized on invalid output");
            return;
        };

        for workspace in monitor.workspaces_mut() {
            workspace.arrange_tiles(animate);
            workspace.refresh();
        }
    }

    /// Return whether this [`Space`] has this [`Output`].
    pub fn has_output(&self, output: &Output) -> bool {
        self.monitors.iter().any(|mon| mon.output() == output)
    }

    /// Get the active [`Workspace`].
    pub fn active_workspace(&self) -> &Workspace {
        let monitor = &self.monitors[self.active_idx];
        monitor.active_workspace()
    }

    /// Get the [`WorkspaceId`] of the active [`Workspace`].
    pub fn active_workspace_id(&self) -> WorkspaceId {
        let monitor = &self.monitors[self.active_idx];
        monitor.active_workspace().id()
    }

    /// Get a mutable reference to the active [`Workspace`] of the active [`Monitor`]
    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        let active_monitor = &mut self.monitors[self.active_idx];
        active_monitor.active_workspace_mut()
    }

    /// Get the active [`Window`] of this [`Space`], if any.
    pub fn active_window(&self) -> Option<Window> {
        let active_monitor = &self.monitors[self.active_idx];
        let active_workspace = active_monitor.active_workspace();
        active_workspace.active_window()
    }

    /// Get a mutable reference to the active [`Tile`] of this [`Space`], if any.
    pub fn active_tile_mut(&mut self) -> Option<&mut tile::Tile> {
        let active_monitor = &mut self.monitors[self.active_idx];
        let active_workspace = active_monitor.active_workspace_mut();
        active_workspace.active_tile_mut()
    }

    /// Get the active [`Monitor`] index of this [`Space`]
    pub fn active_monitor_idx(&self) -> usize {
        self.active_idx
    }

    /// Get the primary [`Monitor`] index of this [`Space`]
    pub fn primary_monitor_idx(&self) -> usize {
        self.primary_idx
    }

    /// Get the active [`Monitor`] of this [`Space`], if any.
    pub fn active_monitor(&self) -> &Monitor {
        &self.monitors[self.active_idx]
    }

    /// Get the active [`Monitor`] of this [`Space`], if any.
    pub fn active_monitor_mut(&mut self) -> &mut Monitor {
        &mut self.monitors[self.active_idx]
    }

    /// Set the active [`Output`]
    pub fn set_active_output(&mut self, output: &Output) -> Option<Window> {
        let Some(idx) = self.monitors.iter().position(|mon| mon.output() == output) else {
            error!("Tried to activate an output that is not tracked by the Space!");
            return None;
        };
        self.active_idx = idx;
        self.monitors[idx].active_workspace().active_window()
    }

    /// Get the active [`Output`].
    pub fn active_output(&self) -> &Output {
        self.monitors[self.active_idx].output()
    }

    /// Get the primary [`Output`].
    pub fn primary_output(&self) -> &Output {
        self.monitors[self.primary_idx].output()
    }

    /// Get the [`Workspace`] associated with this [`WorkspaceId`].
    pub fn workspace_for_id(&self, workspace_id: WorkspaceId) -> Option<&Workspace> {
        self.monitors
            .iter()
            .find_map(|mon| mon.workspaces().find(|ws| ws.id() == workspace_id))
    }

    /// Get the [`Workspace`] associated with this [`WorkspaceId`].
    pub fn workspace_mut_for_id(&mut self, workspace_id: WorkspaceId) -> Option<&mut Workspace> {
        self.monitors
            .iter_mut()
            .find_map(|mon| mon.workspaces_mut().find(|ws| ws.id() == workspace_id))
    }

    /// Get the workspace that has this [`Window`].
    pub fn workspace_for_window(&self, window: &Window) -> Option<&Workspace> {
        self.monitors.iter().find_map(|mon| {
            mon.workspaces()
                .find(|ws| ws.tiles().any(|tile| tile.window() == window))
        })
    }

    /// Get the workspace that has a [`Window`] with this toplevel [`WlSurface`].
    pub fn workspace_for_window_surface(&self, surface: &WlSurface) -> Option<&Workspace> {
        self.monitors.iter().find_map(|mon| {
            mon.workspaces().find(|ws| {
                ws.tiles()
                    .any(|tile| tile.has_surface(surface, WindowSurfaceType::TOPLEVEL))
            })
        })
    }

    /// Get the workspace that has a [`Window`] with this toplevel [`WlSurface`].
    pub fn workspace_mut_for_window_surface(
        &mut self,
        surface: &WlSurface,
    ) -> Option<&mut Workspace> {
        self.monitors.iter_mut().find_map(|mon| {
            mon.workspaces_mut().find(|ws| {
                ws.tiles()
                    .any(|tile| tile.has_surface(surface, WindowSurfaceType::TOPLEVEL))
            })
        })
    }

    /// Get the [`Window`] with this [`WlSurface`] as its toplevel surface
    pub fn find_window(&self, surface: &WlSurface) -> Option<Window> {
        // First check for the window
        if let Some(window) = self
            .interactive_swap
            .as_ref()
            .filter(|swap| swap.tile.window().wl_surface().as_deref() == Some(surface))
            .map(|swap| swap.tile.window().clone())
        {
            return Some(window);
        }

        for monitor in &self.monitors {
            for workspace in monitor.workspaces() {
                for tile in workspace.tiles() {
                    if tile
                        .window()
                        .wl_surface()
                        .is_some_and(|s| s.as_ref() == surface)
                    {
                        return Some(tile.window().clone());
                    }
                }
            }
        }

        None
    }

    /// Get the [`Window`] and a reference [`Workspace`] holding it with this [`WlSurface`] as its
    /// toplevel surface
    pub fn find_window_and_workspace(&self, surface: &WlSurface) -> Option<(Window, &Workspace)> {
        for monitor in &self.monitors {
            for workspace in monitor.workspaces() {
                let mut w = None;
                for tile in workspace.tiles() {
                    if tile
                        .window()
                        .wl_surface()
                        .is_some_and(|s| s.as_ref() == surface)
                    {
                        w = Some(tile.window().clone());
                        break;
                    }
                }

                if let Some(w) = w {
                    return Some((w, workspace));
                }
            }
        }

        None
    }

    /// Get the [`Window`] and a mutable reference to the the [`Workspace`] holding it with
    /// this [`WlSurface`] as its toplevel /// surface
    pub fn find_window_and_workspace_mut(
        &mut self,
        surface: &WlSurface,
    ) -> Option<(Window, &mut Workspace)> {
        for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                let mut w = None;
                for tile in workspace.tiles() {
                    if tile
                        .window()
                        .wl_surface()
                        .is_some_and(|s| s.as_ref() == surface)
                    {
                        w = Some(tile.window().clone());
                        break;
                    }
                }

                if let Some(w) = w {
                    return Some((w, workspace));
                }
            }
        }

        None
    }

    /// Get the [`Output`] holding this window.
    pub fn output_for_surface(&self, surface: &WlSurface) -> Option<&Output> {
        if let Some(swap) = self
            .interactive_swap
            .as_ref()
            .filter(|swap| swap.tile.window().wl_surface().as_deref() == Some(surface))
        {
            // HACK: I really don't know how to handle this properly
            // For now we just use the output that has the tile center.
            let tile_center = swap.tile.geometry().center();
            return swap
                .overlap_outputs
                .iter()
                .find(|output| output.geometry().contains(tile_center));
        }

        for monitor in &self.monitors {
            for workspace in monitor.workspaces() {
                for tile in workspace.tiles() {
                    if tile.has_surface(surface, WindowSurfaceType::ALL) {
                        return Some(monitor.output());
                    }
                }
            }
        }

        None
    }

    /// Activate a [`Window`].
    pub fn activate_window(&mut self, window: &Window, animate: bool) -> bool {
        let mut ret = false;
        let mut new_monitor_idx = None;

        for (monitor_idx, monitor) in self.monitors.iter_mut().enumerate() {
            let mut new_workspace_idx = None;

            for (workspace_idx, workspace) in monitor.workspaces_mut().enumerate() {
                let mut new_tile_idx = None;

                'tiles: for (tile_idx, tile) in workspace.tiles_mut().enumerate() {
                    if tile.window() == window {
                        new_tile_idx = Some(tile_idx);
                        break 'tiles;
                    }
                }

                if let Some(new_tile_idx) = new_tile_idx {
                    workspace.set_active_tile_idx(new_tile_idx);
                    ret = true;
                    new_workspace_idx = Some(workspace_idx)
                }
            }

            if let Some(new_workspace_idx) = new_workspace_idx {
                monitor.set_active_workspace_idx(new_workspace_idx, animate);
                new_monitor_idx = Some(monitor_idx);
            }
        }

        if let Some(new_monitor_idx) = new_monitor_idx {
            self.active_idx = new_monitor_idx;
        }

        ret
    }

    /// Get the location of the window in the global compositor [`Space`]
    pub fn window_location(&self, window: &Window) -> Option<Point<i32, Logical>> {
        for monitor in &self.monitors {
            for workspace in monitor.workspaces() {
                for tile in workspace.tiles() {
                    if tile.window() == window {
                        return Some(
                            tile.location()
                                + tile.window_loc()
                                + workspace.output().current_location(),
                        );
                    }
                }
            }
        }
        None
    }

    /// Select the next [`WorkspaceLayout`](fht_compositor_config::WorkspaceLayout) on the active
    /// [`Monitor`]'s active [`Workspace`].
    ///
    /// See [`Workspace::select_next_layout`]
    pub fn select_next_layout(&mut self, animate: bool) {
        let active_workspace = self.active_workspace_mut();
        active_workspace.select_next_layout(animate);
    }

    /// Select the previous [`WorkspaceLayout`](fht_compositor_config::WorkspaceLayout) on the
    /// active [`Monitor`]'s active [`Workspace`].
    ///
    /// See [`Workspace::select_previous_layout`]
    pub fn select_previous_layout(&mut self, animate: bool) {
        let active_workspace = self.active_workspace_mut();
        active_workspace.select_previous_layout(animate);
    }

    /// Change the master width factor on the active [`Monitor`]'s active [`Workspace`].
    ///
    /// See [`Workspace::change_mwfact`]
    pub fn change_mwfact(&mut self, delta: f64, animate: bool) {
        let active_workspace = self.active_workspace_mut();
        active_workspace.change_mwfact(delta, animate);
    }

    /// Change the number of master clients on the active [`Monitor`]'s active [`Workspace`].
    ///
    /// See [`Workspace::change_nmaster`]
    pub fn change_nmaster(&mut self, delta: i32, animate: bool) {
        let active_workspace = self.active_workspace_mut();
        active_workspace.change_nmaster(delta, animate);
    }

    /// Maximize the [`Tile`] associated with this [`Window`].
    pub fn maximize_window(&mut self, window: &Window, maximize: bool, animate: bool) -> bool {
        for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                let mut arrange = false;
                for tile in workspace.tiles_mut() {
                    if tile.window() == window {
                        window.request_maximized(maximize);
                        arrange = true;
                        break;
                    }
                }

                if arrange {
                    workspace.arrange_tiles(animate);
                    return true;
                }
            }
        }

        false
    }

    /// Float the [`Tile`] associated with this [`Window`].
    pub fn float_window(&mut self, window: &Window, floating: bool, animate: bool) -> bool {
        for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                let mut arrange = false;
                for tile in workspace.tiles_mut() {
                    if tile.window() == window {
                        window.request_tiled(!floating);
                        arrange = true;
                        break;
                    }
                }

                if arrange {
                    workspace.arrange_tiles(animate);
                    return true;
                }
            }
        }

        false
    }

    /// Fullscreen the [`Tile`] associated with this [`Window`].
    pub fn fullscreen_window(&mut self, window: &Window, animate: bool) -> bool {
        for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                if workspace.fullscreen_window(window, animate) {
                    return true;
                }
            }
        }

        false
    }

    /// Prepare the [`Window`] geometry for insertion inside a [`Workspace`].
    ///
    /// Wayland's motto is that "every frame is perfect". Before we sent an initial configure to the
    /// [`Window`], we first need to send it its adequate buffer size in order to be inserted with
    /// its correct size acked.
    pub fn prepare_unconfigured_window(&mut self, window: &Window, workspace_id: WorkspaceId) {
        let Some(workspace) = self.workspace_mut_for_id(workspace_id) else {
            return;
        };

        workspace.prepare_unconfigured_window(window);
    }

    /// Get the fullscreen [`Window`] under the `point`, and its position in global space.
    ///
    /// `point` is expected to be in global coordinate space.
    pub fn fullscreened_window_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(Window, Point<i32, Logical>)> {
        let monitor = self
            .monitors
            .iter()
            .find(|mon| mon.output().geometry().to_f64().contains(point))?;
        let active = monitor.active_workspace();
        Some((active.fullscreened_window()?, Point::default()))
    }

    /// Move this [`Window`] on the active [`Workspace`] of the [`Monitor`] associated with this
    /// output, if any.
    pub fn move_window_to_output(&mut self, window: &Window, output: &Output, animate: bool) {
        // If we try to add the window back into its original workspace.
        let mut original_workspace_id = None;
        'monitors: for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                if workspace.remove_window(window, true) {
                    // We successfully removed the window,
                    // Stop checking for other monitors
                    original_workspace_id = Some(workspace.id());
                    break 'monitors;
                }
            }
        }
        let Some(original_workspace_id) = original_workspace_id else {
            // We did not find the window!? Do not proceed.
            return;
        };

        let Some(target_monitor) = self.monitors.iter_mut().find(|mon| mon.output() == output)
        else {
            // No matching monitor, insert back
            let original_workspace = self
                .workspace_mut_for_id(original_workspace_id)
                .expect("original_workspace_id should always be valid");
            original_workspace.insert_window(window.clone(), animate);
            return;
        };

        let active = target_monitor.active_workspace_mut();
        active.insert_window(window.clone(), animate);
    }

    /// Move this [`Window`] to a given [`Workspace`].
    pub fn move_window_to_workspace(
        &mut self,
        window: &Window,
        workspace_id: WorkspaceId,
        animate: bool,
    ) {
        // If we try to add the window back into its original workspace.
        let mut original_workspace_id = None;
        'monitors: for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                if workspace.remove_window(window, true) {
                    // We successfully removed the window,
                    // Stop checking for other monitors
                    original_workspace_id = Some(workspace.id());
                    break 'monitors;
                }
            }
        }
        let Some(original_workspace_id) = original_workspace_id else {
            // We did not find the window!? Do not proceed.
            return;
        };

        let Some(target_workspace) = self
            .monitors
            .iter_mut()
            .find_map(|mon| mon.workspaces_mut().find(|ws| ws.id() == workspace_id))
        else {
            // No matching monitor, insert back
            let original_workspace = self
                .workspace_mut_for_id(original_workspace_id)
                .expect("original_workspace_id should always be valid");
            original_workspace.insert_window(window.clone(), animate);
            return;
        };

        target_workspace.insert_window(window.clone(), animate);
    }

    /// Get the fullscreen [`Window`] under the `point`, and its position in global space.
    ///
    /// `point` is expected to be in global coordinate space.
    pub fn window_under(
        &self,
        mut point: Point<f64, Logical>,
    ) -> Option<(Window, Point<i32, Logical>)> {
        let monitor = self
            .monitors
            .iter()
            .find(|mon| mon.output().geometry().to_f64().contains(point))?;
        point -= monitor.output().current_location().to_f64(); // make relative to output
        let active = monitor.active_workspace();

        for tile in active.tiles_in_render_order() {
            let window = tile.window();
            let loc = tile.location() + tile.window_loc();
            let bbox = {
                let mut bbox = window.bbox();
                bbox.loc += loc;
                bbox
            };
            let render_location = loc - window.render_offset();
            if bbox.to_f64().contains(point)
                && window
                    .surface_under(point - render_location.to_f64(), WindowSurfaceType::ALL)
                    .is_some()
            {
                return Some((window.clone(), render_location));
            }
        }

        None
    }

    /// Change the proportion of the [`Tile`] associated with this [`Window`]
    pub fn change_proportion(&mut self, window: &Window, delta: f64, animate: bool) {
        for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                let mut arrange = false;
                for tile in workspace.tiles_mut() {
                    if tile.window() == window {
                        let proportion = (tile.proportion() + delta).max(0.01);
                        tile.set_proportion(proportion);
                        arrange = true;
                        break;
                    }
                }

                if arrange {
                    workspace.arrange_tiles(animate);
                    return;
                }
            }
        }
    }

    /// Center a given window.
    pub fn center_window(&mut self, window: &Window, animate: bool) {
        if window.tiled() {
            return;
        }

        for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                let mut arrange = false;
                let output_geometry = workspace.output().geometry();
                for tile in workspace.tiles_mut() {
                    if tile.window() == window {
                        let size = tile.size();
                        tile.set_location(output_geometry.center() - size.downscale(2), animate);
                        arrange = true;
                        break;
                    }
                }

                if arrange {
                    workspace.arrange_tiles(animate);
                    return;
                }
            }
        }
    }

    /// Start an interactive swap in the [`Workspace`] of this [`Window`].
    ///
    /// Returns [`true`] if the grab got started inside the [`Workspace`].
    pub fn start_interactive_swap(
        &mut self,
        window: &Window,
        pointer_loc: Point<i32, Logical>,
    ) -> bool {
        if self.interactive_swap.is_some() {
            return false;
        }

        for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                if let Some(mut tile) = workspace.start_interactive_swap(window) {
                    // First move the tile instantly to global space
                    tile.set_location(
                        tile.location() + workspace.output().current_location(),
                        false,
                    );

                    // Make the tile slightly smaller, just for aesthetic urposes and give a visual
                    // clue that we grabbed it and is not in a swap state.
                    if tile.window().tiled()
                        || workspace.current_layout()
                            != fht_compositor_config::WorkspaceLayout::Floating
                    {
                        let new_size = tile.size().to_f64().upscale(0.8).to_i32_round();
                        let new_loc = pointer_loc - new_size.downscale(2);
                        tile.set_geometry(Rectangle::new(new_loc, new_size), true);
                    } else {
                        tile.set_location(pointer_loc - tile.size().downscale(2), true);
                    }

                    let output = workspace.output().clone();
                    self.interactive_swap = Some(InteractiveSwap {
                        tile,
                        overlap_outputs: vec![output],
                    });
                    return true;
                }
            }
        }

        false
    }

    /// Handle the iteractive swap motion for this window.
    ///
    /// Returns [`true`] if the grab should continue.
    pub fn handle_interactive_swap_motion(
        &mut self,
        window: &Window,
        pointer_loc: Point<i32, Logical>,
    ) -> bool {
        let Some(interactive_swap) = &mut self.interactive_swap else {
            return false;
        };

        if interactive_swap.tile.window() != window {
            return false;
        }

        let new_location = pointer_loc - interactive_swap.tile.visual_size().downscale(2);
        interactive_swap.tile.set_location(new_location, false);

        // Now, update the outputs the tile is overlapping with.
        let new_geometry = interactive_swap.tile.geometry();
        interactive_swap.overlap_outputs = self
            .monitors
            .iter()
            .map(|mon| mon.output())
            .filter(|o| o.geometry().intersection(new_geometry).is_some())
            .cloned()
            .collect();

        true
    }

    /// Handle the iteractive swap motion for this window.
    ///
    /// Returns [`true`] if the grab should continue.
    pub fn handle_interactive_swap_end(
        &mut self,
        window: &Window,
        cursor_position: Point<f64, Logical>,
    ) {
        let Some(mut interactive_swap) = self.interactive_swap.take() else {
            return;
        };

        if interactive_swap.tile.window() != window {
            return;
        }

        let monitor_under_idx = self
            .monitors
            .iter_mut()
            .position(|mon| {
                mon.output()
                    .geometry()
                    .contains(cursor_position.to_i32_round())
            })
            .expect("Cursor position out of space!");
        let monitor_under = &mut self.monitors[monitor_under_idx];
        let output_loc = monitor_under.output().current_location();
        // Move the tile to the correct position relative to the output so that animation doesn't
        // break, since handle_interactive_swap_motion sets the absolute position
        interactive_swap
            .tile
            .set_location(interactive_swap.tile.visual_location() - output_loc, false);
        monitor_under
            .active_workspace_mut()
            .insert_tile_with_cursor_position(
                interactive_swap.tile,
                cursor_position.to_i32_round() - output_loc,
            );
        self.active_idx = monitor_under_idx;
    }

    /// Renders the tile affected by the current interactive swap.
    pub fn render_interactive_swap<R: FhtRenderer>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        scale: i32,
    ) -> Vec<RelocateRenderElement<TileRenderElement<R>>> {
        let Some(interactive_swap) = self.interactive_swap.as_mut() else {
            return vec![];
        };

        if !interactive_swap.overlap_outputs.contains(output) {
            return vec![];
        }

        // Usually, the tile's location is local, but in our case it is global due to how
        // Space::handle_interactive_swap_motion is done.
        // We just have to offset by the output location to render it accurately.
        let output_loc = output.current_location().to_physical(scale);

        interactive_swap
            .tile
            .render(renderer, scale, 1.0, output, Point::default())
            .map(|element| {
                RelocateRenderElement::from_element(
                    element,
                    output_loc.upscale(-1),
                    Relocate::Relative,
                )
            })
            .collect()
    }

    /// Start an interactive resize in the [`Workspace`] of this [`Window`].
    ///
    /// Returns [`true`] if the grab got started inside the [`Workspace`].
    pub fn start_interactive_resize(&mut self, window: &Window, edges: ResizeEdge) -> bool {
        for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                if workspace.start_interactive_resize(window, edges) {
                    return true;
                }
            }
        }

        false
    }

    /// Handle the iteractive resize motion for this window.
    ///
    /// Returns [`true`] if the grab should continue.
    pub fn handle_interactive_resize_motion(
        &mut self,
        window: &Window,
        delta: Point<i32, Logical>,
    ) -> bool {
        for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                if workspace.handle_interactive_resize_motion(window, delta) {
                    return true;
                }
            }
        }

        false
    }

    /// Handle the iteractive resize motion for this window.
    ///
    /// Returns [`true`] if the grab should continue.
    pub fn handle_interactive_resize_end(
        &mut self,
        window: &Window,
        position: Point<f64, Logical>,
    ) {
        for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                let position_in_workspace =
                    position - workspace.output().current_location().to_f64();
                if workspace.handle_interactive_resize_end(window, position_in_workspace) {
                    return;
                }
            }
        }
    }
}

/// Configuration for the workspace system derived from the compositor configuration.
///
/// In an animation field is None, this means the animation is disabled.
#[derive(Debug)]
pub struct Config {
    pub workspace_switch_animation: Option<(
        AnimationConfig,
        fht_compositor_config::WorkspaceSwitchAnimationDirection,
    )>,
    pub window_geometry_animation: Option<AnimationConfig>,
    pub window_open_close_animation: Option<AnimationConfig>,
    pub border_animation: Option<AnimationConfig>,
    pub shadow: fht_compositor_config::Shadow,
    pub insert_window_strategy: fht_compositor_config::InsertWindowStrategy,
    pub border: fht_compositor_config::Border,
    pub layouts: Vec<fht_compositor_config::WorkspaceLayout>,
    pub nmaster: usize,
    pub gaps: (i32, i32),
    pub mwfact: f64,
    pub focus_new_windows: bool,
    pub blur: fht_compositor_config::Blur,
}

impl Config {
    fn new(config: &fht_compositor_config::Config) -> anyhow::Result<Self> {
        Ok(Self {
            workspace_switch_animation: AnimationConfig::new(
                config.animations.workspace_switch.duration,
                config.animations.workspace_switch.curve,
                !config.animations.disable && !config.animations.workspace_switch.disable,
            )
            .map(|a| (a, config.animations.workspace_switch.direction)),
            window_geometry_animation: AnimationConfig::new(
                config.animations.window_geometry.duration,
                config.animations.window_geometry.curve,
                !config.animations.disable && !config.animations.window_geometry.disable,
            ),
            window_open_close_animation: AnimationConfig::new(
                config.animations.window_open_close.duration,
                config.animations.window_open_close.curve,
                !config.animations.disable && !config.animations.window_open_close.disable,
            ),
            border_animation: AnimationConfig::new(
                config.animations.border.duration,
                config.animations.border.curve,
                !config.animations.disable && !config.animations.border.disable,
            ),
            shadow: config.decorations.shadow,
            insert_window_strategy: config.general.insert_window_strategy,
            focus_new_windows: config.general.focus_new_windows,
            layouts: config.general.layouts.clone(),
            nmaster: config.general.nmaster.get(),
            gaps: (config.general.outer_gaps, config.general.inner_gaps),
            mwfact: config.general.mwfact,
            border: config.decorations.border,
            blur: config.decorations.blur,
        })
    }
}

#[derive(Debug)]
pub struct AnimationConfig {
    pub duration: Duration,
    pub curve: AnimationCurve,
}

impl AnimationConfig {
    const DISABLED: Self = Self {
        duration: Duration::ZERO,
        curve: AnimationCurve::Simple(fht_animation::curve::Easing::Linear),
    };

    pub fn new(duration: Duration, curve: AnimationCurve, enable: bool) -> Option<Self> {
        enable.then_some(Self { duration, curve })
    }
}
