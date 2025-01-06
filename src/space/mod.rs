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
use smithay::desktop::WindowSurfaceType;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};
use smithay::wayland::seat::WaylandFocus;
pub use workspace::{Workspace, WorkspaceId, WorkspaceRenderElement};

use crate::input::resize_tile_grab::ResizeEdge;
use crate::output::OutputExt;
use crate::window::Window;

mod closing_tile;
mod decorations;
mod monitor;
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

    /// The index of the active [`Monitoir`].
    ///
    /// This should be the monitor that has the pointer cursor in its bounds.
    active_idx: usize,

    /// Shared configuration with across the workspace system.
    config: Rc<Config>,
}

impl Space {
    /// Create a new [`Space`].
    pub fn new(config: &fht_compositor_config::Config) -> Self {
        let config = Config::new(config).expect("Space configuration invariants!");
        Self {
            monitors: vec![],
            primary_idx: 0,
            active_idx: 0,
            config: Rc::new(config),
        }
    }

    /// Run periodic clean-up tasks.
    pub fn refresh(&mut self) {
        crate::profile_function!();
        for (idx, monitor) in self.monitors.iter_mut().enumerate() {
            monitor.refresh(idx == self.active_idx)
        }
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
    pub fn monitors(&self) -> impl Iterator<Item = &Monitor> + ExactSizeIterator {
        self.monitors.iter()
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
        self.monitors
            .iter()
            .find(|mon| mon.output() == output)
            .into_iter()
            .flat_map(|mon| mon.visible_windows())
    }

    /// Get the [`Window`]s on the associated [`Output`].
    pub fn windows_on_output(&self, output: &Output) -> impl Iterator<Item = &Window> {
        self.monitors
            .iter()
            .find(|mon| mon.output() == output)
            .into_iter()
            .flat_map(Monitor::workspaces)
            .flat_map(Workspace::windows)
    }

    /// Get an iterator of all the [`Output`]s managed by this [`Space`].
    pub fn outputs(&self) -> impl Iterator<Item = &Output> + ExactSizeIterator {
        self.monitors.iter().map(Monitor::output)
    }

    /// Get an iterator of all the [`Windows`]s managed by this [`Space`].
    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.monitors
            .iter()
            .flat_map(Monitor::workspaces)
            .flat_map(Workspace::windows)
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
            // TODO: Handle empty monitors more gracefully, for example with a laptop, when the main
            // screens gets disabled/turn off (IE entering sleep/hibernation).
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

    /// Get the active [`Window`] of this [`Space`], if any.
    pub fn active_monitor(&self) -> &Monitor {
        &self.monitors[self.active_idx]
    }

    /// Get the active [`Window`] of this [`Space`], if any.
    pub fn active_monitor_mut(&mut self) -> &mut Monitor {
        &mut self.monitors[self.active_idx]
    }

    /// Set the active [`Output`]
    pub fn set_active_output(&mut self, output: &Output) {
        let Some(idx) = self.monitors.iter().position(|mon| mon.output() == output) else {
            error!("Tried to activate an output that is not tracked by the Space!");
            return;
        };
        self.active_idx = idx;
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

    /// Get the location of the window in the global compositor [`Space`]
    pub fn window_visual_location(&self, window: &Window) -> Option<Point<i32, Logical>> {
        for monitor in &self.monitors {
            for workspace in monitor.workspaces() {
                for tile in workspace.tiles() {
                    if tile.window() == window {
                        return Some(
                            tile.visual_location()
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
    pub fn fullscreened_window(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(Window, Point<i32, Logical>)> {
        let monitor = self
            .monitors
            .iter()
            .find(|mon| mon.output().geometry().to_f64().contains(point))?;
        let active = monitor.active_workspace();
        Some((
            active.fullscreened_window()?,
            active.output().current_location(),
        ))
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

    /// Get the fullscreen [`Window`] under the `point`, and its position in global space.
    ///
    /// `point` is expected to be in global coordinate space.
    pub fn window_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(Window, Point<i32, Logical>)> {
        let monitor = self
            .monitors
            .iter()
            .find(|mon| mon.output().geometry().to_f64().contains(point))?;
        let active = monitor.active_workspace();

        // Fullscreened tile always get priority
        if let Some(tile) = active.fullscreened_tile() {
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

        // Active tile is always above everything else
        if let Some(tile) = active.active_tile() {
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

        for tile in active.tiles() {
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

    /// Start an interactive swap in the [`Workspace`] of this [`Window`].
    ///
    /// Returns [`true`] if the grab got started inside the [`Workspace`].
    pub fn start_interactive_swap(&mut self, window: &Window) -> bool {
        for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                if workspace.start_interactive_swap(window) {
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
        delta: Point<i32, Logical>,
    ) -> bool {
        for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                if workspace.handle_interactive_swap_motion(window, delta) {
                    return true;
                }
            }
        }

        false
    }

    /// Handle the iteractive swap motion for this window.
    ///
    /// Returns [`true`] if the grab should continue.
    pub fn handle_interactive_swap_end(&mut self, window: &Window, position: Point<f64, Logical>) {
        for monitor in &mut self.monitors {
            for workspace in monitor.workspaces_mut() {
                let position_in_workspace =
                    position - workspace.output().current_location().to_f64();
                if workspace.handle_interactive_swap_end(window, position_in_workspace) {
                    return;
                }
            }
        }
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
    pub shadow: Option<fht_compositor_config::Shadow>,
    pub insert_window_strategy: fht_compositor_config::InsertWindowStrategy,
    pub border: fht_compositor_config::Border,
    pub layouts: Vec<fht_compositor_config::WorkspaceLayout>,
    pub nmaster: usize,
    pub gaps: (i32, i32),
    pub mwfact: f64,
    pub focus_new_windows: bool,
}

impl Config {
    pub fn check_invariants(config: &fht_compositor_config::Config) -> anyhow::Result<()> {
        if config.general.nmaster == 0 {
            anyhow::bail!("general.nmaster cannot be zero!");
        }
        if config.general.mwfact < 0.01 || config.general.mwfact > 0.99 {
            anyhow::bail!("general.mwfact must be between 0.01 and 0.99")
        }
        if config.general.layouts.is_empty() {
            anyhow::bail!("general.layouts must never be empty!");
        }
        Ok(())
    }

    fn new(config: &fht_compositor_config::Config) -> anyhow::Result<Self> {
        Self::check_invariants(config)?;
        Ok(Self {
            workspace_switch_animation: AnimationConfig::new(
                config.animations.workspace_switch.duration,
                config.animations.workspace_switch.curve,
                config.animations.workspace_switch.disable,
            )
            .map(|a| (a, config.animations.workspace_switch.direction)),
            window_geometry_animation: AnimationConfig::new(
                config.animations.window_geometry.duration,
                config.animations.window_geometry.curve,
                config.animations.window_geometry.disable,
            ),
            window_open_close_animation: AnimationConfig::new(
                config.animations.window_open_close.duration,
                config.animations.window_open_close.curve,
                config.animations.window_open_close.disable,
            ),
            shadow: (!config.decorations.shadow.disable).then(|| config.decorations.shadow),
            insert_window_strategy: config.general.insert_window_strategy,
            focus_new_windows: config.general.focus_new_windows,
            layouts: config.general.layouts.clone(),
            nmaster: config.general.nmaster,
            gaps: (config.general.outer_gaps, config.general.inner_gaps),
            mwfact: config.general.mwfact,
            border: config.decorations.border,
        })
    }
}

#[derive(Debug)]
pub struct AnimationConfig {
    pub duration: Duration,
    pub curve: AnimationCurve,
}

impl AnimationConfig {
    pub fn new(duration: Duration, curve: AnimationCurve, disable: bool) -> Option<Self> {
        (!disable).then(|| Self { duration, curve })
    }
}
