//! Workspace logic code.
//!
//! `fht-compositor` has a static workspace scheme, as in each output gets a fixed number of
//! workspaces to work with using a [`WorkspaceSet`], with each workspace having a unique dynamic
//! layout that tiles the windows inside the available space.
//!
//! Each workspace holds a number of [`WorkspaceTile`]s, which is a generic abstraction over an
//! element that implements [`WorkspaceElement`], that could be windows, textures, buffers, etc.
//!
//! `fht-compositor` holds then the following rules when managing workspaces and windows
//!
//! 1. Each unique workspace can only exist in one single set
//! 2. Each workspace set can only be assigned to one single output
//! 3. Each tile can only exist in a single unique workspace
//!
//! When unplugging an output, all the tiles from its workspaces get inserted into the workspaces
//! of the active output, matching the workspace index between the removed and the active output
//! workspace set.

pub mod layout;
pub mod tile;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use layout::Layout;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::desktop::WindowSurfaceType;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Monotonic, Physical, Point, Rectangle, Scale, Time};

pub use self::layout::WorkspaceLayout;
use self::tile::{Tile, WorkspaceTileRenderElement};
use crate::config::{
    BorderConfig, InsertWindowStrategy, WorkspaceSwitchAnimationDirection, CONFIG,
};
use crate::fht_render_elements;
use crate::renderer::FhtRenderer;
use crate::utils::animation::Animation;
use crate::utils::output::OutputExt;
use crate::window::Window;

pub struct WorkspaceSet {
    output: Output,
    workspaces: Vec<Workspace>,
    switch_animation: Option<WorkspaceSwitchAnimation>,
    active_idx: usize,
}

#[allow(dead_code)]
impl WorkspaceSet {
    pub fn new(output: Output) -> Self {
        let workspaces = (0..9).map(|_| Workspace::new(output.clone())).collect();
        Self {
            output: output.clone(),
            workspaces,
            switch_animation: None,
            active_idx: 0,
        }
    }

    pub fn output(&self) -> Output {
        self.output.clone()
    }

    pub fn refresh(&mut self) {
        self.workspaces_mut().for_each(Workspace::refresh);
    }

    pub fn reload_config(&mut self) {
        for workspace in &mut self.workspaces {
            workspace.layout = Layout::new(
                &self.output,
                CONFIG.general.layouts.clone(),
                CONFIG.general.nmaster,
                CONFIG.general.mwfact,
                CONFIG.general.inner_gaps,
                CONFIG.general.outer_gaps,
            );
        }
    }

    pub fn set_active_idx(&mut self, target_idx: usize, animate: bool) -> Option<Window> {
        let target_idx = target_idx.clamp(0, 9);
        if !animate {
            self.active_idx = target_idx;
            return self.workspaces[target_idx].focused();
        }

        let active_idx = self.active_idx;
        if target_idx == active_idx || self.switch_animation.is_some() {
            return None;
        }

        self.switch_animation = Some(WorkspaceSwitchAnimation::new(target_idx));
        self.workspaces[target_idx].focused()
    }

    pub fn get_active_idx(&self) -> usize {
        if let Some(WorkspaceSwitchAnimation { target_idx, .. }) = self.switch_animation.as_ref() {
            *target_idx
        } else {
            self.active_idx
        }
    }

    pub fn merge_with(&mut self, other: Self) {
        // Current behaviour:
        //
        // Move each window from each workspace in this removed output wset and bind it to the
        // first output available, very simple.
        //
        // In other words, if you had a window on ws1, 4, and 8 on this output, they would get
        // moved to their respective workspace on the first available wset.
        for (ws, other_ws) in self.workspaces_mut().zip(other.workspaces) {
            ws.merge_with(other_ws);
        }
    }

    pub fn get_workspace(&self, idx: usize) -> &Workspace {
        &self.workspaces[idx]
    }

    pub fn get_workspace_mut(&mut self, idx: usize) -> &mut Workspace {
        &mut self.workspaces[idx]
    }

    pub fn active(&self) -> &Workspace {
        if let Some(WorkspaceSwitchAnimation { target_idx, .. }) = self.switch_animation.as_ref() {
            &self.workspaces[*target_idx]
        } else {
            &self.workspaces[self.active_idx]
        }
    }

    pub fn active_mut(&mut self) -> &mut Workspace {
        if let Some(WorkspaceSwitchAnimation { target_idx, .. }) = self.switch_animation.as_ref() {
            &mut self.workspaces[*target_idx]
        } else {
            &mut self.workspaces[self.active_idx]
        }
    }

    pub fn workspaces(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces.iter()
    }

    pub fn workspaces_mut(&mut self) -> impl Iterator<Item = &mut Workspace> {
        self.workspaces.iter_mut()
    }

    pub fn arrange(&mut self) {
        self.workspaces_mut().for_each(|ws| ws.arrange_tiles(true))
    }

    pub fn output_resized(&mut self) {
        self.workspaces_mut().for_each(|ws| ws.output_resized())
    }

    pub fn find_window(&self, surface: &WlSurface) -> Option<Window> {
        self.workspaces()
            .find_map(|ws| ws.find_tile(surface).map(Tile::window))
            .cloned()
    }

    pub fn find_tile_mut(&mut self, surface: &WlSurface) -> Option<&mut Tile> {
        self.workspaces_mut()
            .find_map(|ws| ws.find_tile_mut(surface))
    }

    pub fn find_workspace(&self, surface: &WlSurface) -> Option<&Workspace> {
        self.workspaces().find(|ws| ws.has_surface(surface))
    }

    pub fn find_workspace_mut(&mut self, surface: &WlSurface) -> Option<&mut Workspace> {
        self.workspaces_mut().find(|ws| ws.has_surface(surface))
    }

    pub fn find_window_and_workspace(&self, surface: &WlSurface) -> Option<(Window, &Workspace)> {
        self.workspaces().find_map(|ws| {
            let window = ws.find_tile(surface).map(|w| w.window().clone())?;
            Some((window, ws))
        })
    }

    pub fn find_window_and_workspace_mut(
        &mut self,
        surface: &WlSurface,
    ) -> Option<(Window, &mut Workspace)> {
        self.workspaces_mut().find_map(|ws| {
            let window = ws.find_tile(surface).map(|w| w.window().clone())?;
            Some((window, ws))
        })
    }

    pub fn visible_windows(&self) -> impl Iterator<Item = &Window> + '_ {
        let switching_windows = self
            .switch_animation
            .as_ref()
            .map(|anim| {
                let ws = &self.workspaces[anim.target_idx];

                ws.fullscreen
                    .as_ref()
                    .map(|fs| fs.inner.window())
                    .into_iter()
                    .chain(ws.tiles.iter().map(Tile::window))
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .flatten();

        let active = self.active();
        active
            .fullscreen
            .as_ref()
            .map(|fs| fs.inner.window())
            .into_iter()
            .chain(active.tiles.iter().map(Tile::window))
            .chain(switching_windows)
    }

    pub fn workspace_for_window(&self, window: &Window) -> Option<&Workspace> {
        self.workspaces().find(|ws| ws.has_window(window))
    }

    pub fn workspace_mut_for_window(&mut self, window: &Window) -> Option<&mut Workspace> {
        self.workspaces_mut().find(|ws| ws.has_window(window))
    }

    #[profiling::function]
    pub fn current_fullscreen(&self) -> Option<(Window, Point<i32, Logical>)> {
        let Some(animation) = self.switch_animation.as_ref() else {
            return self.active().fullscreen.as_ref().map(|fs| {
                // Fullscreen is always at (0,0)
                (fs.inner.window().clone(), (0, 0).into())
            });
        };

        let output_geo = self.output.geometry();
        let (current_offset, target_offset) =
            animation.calculate_offsets(self.active_idx, output_geo);
        self.active()
            .fullscreen
            .as_ref()
            .map(|fs| (fs.inner.window().clone(), current_offset))
            .or_else(|| {
                self.workspaces[animation.target_idx]
                    .fullscreen
                    .as_ref()
                    .map(|fs| (fs.inner.window().clone(), target_offset))
            })
    }

    #[profiling::function]
    pub fn window_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(Window, Point<i32, Logical>)> {
        let Some(animation) = self.switch_animation.as_ref() else {
            // It's just the active one, so no need to do additional calculations.
            return self.active().window_under(point);
        };

        let output_geo = self.output.geometry();
        let (current_offset, target_offset) =
            animation.calculate_offsets(self.active_idx, output_geo);

        self.active()
            .window_under(point + current_offset.to_f64())
            .map(|(ft, loc)| (ft, loc + current_offset))
            .or_else(|| {
                self.workspaces[animation.target_idx]
                    .window_under(point + target_offset.to_f64())
                    .map(|(ft, loc)| (ft, loc + target_offset))
            })
    }

    pub fn has_switch_animation(&self) -> bool {
        self.switch_animation.is_some()
    }

    pub fn advance_animations(&mut self, current_time: Time<Monotonic>) -> bool {
        let mut ret = false;

        if let Some(WorkspaceSwitchAnimation { target_idx, .. }) =
            self.switch_animation.take_if(|a| a.animation.is_finished())
        {
            self.active_idx = target_idx;
        }
        if let Some(animation) = self.switch_animation.as_mut() {
            animation.animation.set_current_time(current_time);
            ret = true;
        }

        for ws in self.workspaces_mut() {
            if let Some(FullscreenTile { inner, .. }) = ws.fullscreen.as_mut() {
                ret |= inner.advance_animations(current_time);
            }

            for window in &mut ws.tiles {
                ret |= window.advance_animations(current_time);
            }
        }

        ret
    }

    #[profiling::function]
    pub fn render_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
    ) -> (bool, Vec<WorkspaceSetRenderElement<R>>) {
        let mut elements = vec![];
        let active = &self.workspaces[self.active_idx];
        let output_geo: Rectangle<i32, Physical> =
            self.output.geometry().to_physical_precise_round(scale);

        // No switch, just give what's active.
        let active_elements = active.render_elements(renderer, scale);
        let Some(animation) = self.switch_animation.as_ref() else {
            elements.extend(
                active_elements
                    .into_iter()
                    .map(WorkspaceSetRenderElement::Normal),
            );

            return (active.fullscreen.is_some(), elements);
        };

        // Switching
        let target = &self.workspaces[animation.target_idx];
        let target_elements = target.render_elements(renderer, scale);

        // Switch finished, avoid blank frame and return target elements immediatly
        if animation.animation.is_finished() {
            elements = target_elements
                .into_iter()
                .map(WorkspaceSetRenderElement::Normal)
                .collect();
            return (target.fullscreen.is_some(), elements);
        }

        let (current_offset, target_offset) =
            animation.calculate_offsets(self.active_idx, output_geo);
        elements.extend(active_elements.into_iter().filter_map(|element| {
            let relocate =
                RelocateRenderElement::from_element(element, current_offset, Relocate::Relative);
            // FIXME: This makes the border look funky. Should go figure out why
            // let crop = CropRenderElement::from_element(relocate, scale, output_geo)?;
            Some(WorkspaceSetRenderElement::Switching(relocate))
        }));
        elements.extend(target_elements.into_iter().filter_map(|element| {
            let relocate =
                RelocateRenderElement::from_element(element, target_offset, Relocate::Relative);
            // FIXME: This makes the border look funky. Should go figure out why
            // let crop = CropRenderElement::from_element(relocate, scale, output_geo)?;
            Some(WorkspaceSetRenderElement::Switching(relocate))
        }));

        (
            active.fullscreen.is_some() || target.fullscreen.is_some(),
            elements,
        )
    }
}

pub struct WorkspaceSwitchAnimation {
    pub animation: Animation,
    pub target_idx: usize,
}

impl WorkspaceSwitchAnimation {
    fn new(target_idx: usize) -> Self {
        // When going to the next workspace, the values describes the offset of the next workspace.
        // When going to the previous workspace, the values describe the offset of the current
        // workspace

        let animation = Animation::new(
            0.0,
            1.0,
            CONFIG.animation.workspace_switch.curve,
            Duration::from_millis(CONFIG.animation.workspace_switch.duration),
        )
        .expect("Should never fail!");

        Self {
            animation,
            target_idx,
        }
    }

    fn calculate_offsets<Kind>(
        &self,
        active_idx: usize,
        area: Rectangle<i32, Kind>,
    ) -> (Point<i32, Kind>, Point<i32, Kind>) {
        let value = self.animation.value();
        if self.target_idx > active_idx {
            // Focusing the next offset.
            // For the active, how much should we *remove* from the current position
            // For the target, how much should we add to the current position
            match CONFIG.animation.workspace_switch.direction {
                WorkspaceSwitchAnimationDirection::Horizontal => {
                    let offset = (value * area.size.w as f64).round() as i32;
                    (
                        Point::from(((-offset), 0)),
                        Point::from(((-offset + area.size.w), 0)),
                    )
                }
                WorkspaceSwitchAnimationDirection::Vertical => {
                    let offset = (value * area.size.h as f64).round() as i32;
                    (
                        Point::from((0, (-offset))),
                        Point::from((0, (-offset + area.size.h))),
                    )
                }
            }
        } else {
            // Focusing a previous workspace
            // For the active, how much should we add to tyhe current position
            // For the target, how much should we remove from the current position.
            match CONFIG.animation.workspace_switch.direction {
                WorkspaceSwitchAnimationDirection::Horizontal => {
                    let offset = (value * area.size.w as f64).round() as i32;
                    (
                        Point::from((offset, 0)),
                        Point::from((offset - area.size.w, 0)),
                    )
                }
                WorkspaceSwitchAnimationDirection::Vertical => {
                    let offset = (value * area.size.h as f64).round() as i32;
                    (
                        Point::from((0, (offset))),
                        Point::from((0, (offset - area.size.h))),
                    )
                }
            }
        }
    }
}

fht_render_elements! {
    WorkspaceSetRenderElement<R> => {
        Normal = WorkspaceTileRenderElement<R>,
        Switching = RelocateRenderElement<WorkspaceTileRenderElement<R>>,
    }
}

static WORKSPACE_IDS: AtomicUsize = AtomicUsize::new(0);
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct WorkspaceId(usize);
impl WorkspaceId {
    pub fn unique() -> Self {
        Self(WORKSPACE_IDS.fetch_add(1, Ordering::SeqCst))
    }
}

pub struct Workspace {
    output: Output,
    tiles: Vec<Tile>,
    focused_tile_idx: usize,
    layout: Layout,
    fullscreen: Option<FullscreenTile>,
    id: WorkspaceId,
}

impl Workspace {
    pub fn new(output: Output) -> Self {
        let layout = Layout::new(
            &output,
            CONFIG.general.layouts.clone(),
            CONFIG.general.nmaster,
            CONFIG.general.mwfact,
            CONFIG.general.inner_gaps,
            CONFIG.general.outer_gaps,
        );
        Self {
            output,
            tiles: vec![],
            focused_tile_idx: 0,
            layout,
            fullscreen: None,
            id: WorkspaceId::unique(),
        }
    }

    pub fn output(&self) -> Output {
        self.output.clone()
    }

    pub fn id(&self) -> WorkspaceId {
        self.id
    }

    pub fn merge_with(&mut self, mut other: Self) {
        if let Some(fullscreen) = other.fullscreen.take() {
            let (window, border_config) = fullscreen.inner.into_window();
            self.insert_window(window, border_config, true);
        }

        for (window, border_config) in other.tiles.into_iter().map(Tile::into_window) {
            self.insert_window(window, border_config, true);
        }
    }

    pub fn tiles(&self) -> impl Iterator<Item = &Tile> {
        self.tiles.iter()
    }

    #[profiling::function]
    pub fn refresh(&mut self) {
        let output_geometry = self.output.geometry();

        let mut should_refresh_geometries =
            self.fullscreen.take_if(|fs| !fs.inner.alive()).is_some();

        if self
            .fullscreen
            .as_ref()
            .is_some_and(|fs| !fs.inner.window().fullscreen())
        {
            let FullscreenTile {
                inner,
                last_known_idx,
            } = self.take_fullscreen().unwrap();
            self.tiles.insert(last_known_idx, inner);
            should_refresh_geometries = true;
        }

        if let Some(fullscreen) = self.fullscreen.as_mut() {
            // This is now managed globally with focus targets
            fullscreen.inner.window().request_activated(true);

            let mut bbox = fullscreen.inner.window().bbox();
            bbox.loc = fullscreen.inner.location() + output_geometry.loc;
            if let Some(mut overlap) = output_geometry.intersection(bbox) {
                // output_enter excepts the overlap to be relative to the window, weird choice
                // but I comply.
                overlap.loc -= bbox.loc;
                fullscreen
                    .inner
                    .window()
                    .enter_output(&self.output, overlap);
            }

            fullscreen.inner.send_pending_configure();
            fullscreen.inner.window().refresh();
        }

        // Clean dead/zombie tiles
        let old_len = self.tiles.len();
        self.tiles.retain(IsAlive::alive);
        let new_len = self.tiles.len();
        should_refresh_geometries |= new_len != old_len;

        if should_refresh_geometries {
            self.focused_tile_idx = self.focused_tile_idx.clamp(0, new_len.saturating_sub(1));
            self.arrange_tiles(true);
        }

        // Refresh internal state of windows
        for (idx, tile) in self.tiles.iter_mut().enumerate() {
            // This is now managed globally with focus targets
            tile.window()
                .request_activated(idx == self.focused_tile_idx);

            let mut bbox = tile.window().bbox();
            bbox.loc = tile.location() + output_geometry.loc;

            if let Some(mut overlap) = output_geometry.intersection(bbox) {
                // output_enter excepts the overlap to be relative to the window, weird choice
                // but I comply.
                overlap.loc -= bbox.loc;
                tile.window().enter_output(&self.output, overlap);
            }

            tile.send_pending_configure();
            tile.window().refresh();
        }
    }

    #[profiling::function]
    pub fn render_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
    ) -> Vec<WorkspaceTileRenderElement<R>> {
        let mut render_elements = vec![];

        // If we have a fullscreen, render it and off we go.
        if let Some(FullscreenTile { inner, .. }) = self.fullscreen.as_ref() {
            return inner
                .render_elements(
                    renderer,
                    scale,
                    CONFIG.decoration.focused_window_opacity,
                    true,
                )
                .collect();
        }

        if self.tiles.is_empty() {
            return render_elements;
        }

        if let Some(tile) = self.focused_tile() {
            render_elements.extend(tile.render_elements(
                renderer,
                scale,
                CONFIG.decoration.focused_window_opacity,
                true,
            ));
        }

        for (idx, tile) in self.tiles().enumerate() {
            if idx == self.focused_tile_idx {
                continue;
            }

            let elements = tile.render_elements(
                renderer,
                scale,
                CONFIG.decoration.normal_window_opacity,
                false,
            );
            render_elements.extend(elements);
        }

        render_elements
    }
}

// Inserting and removing elements
impl Workspace {
    pub fn insert_window(
        &mut self,
        window: Window,
        border_config: Option<BorderConfig>,
        animate: bool,
    ) {
        if self.has_window(&window) {
            return;
        }

        window.request_bounds(Some(self.output.geometry().size));
        window.configure_for_output(&self.output);
        let mut tile = Tile::new(window.clone(), border_config);
        tile.start_opening_animation();

        // NOTE: In the following code we dont call to send_pending_configure since arrange_tiles
        // does this for us automatically.

        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
        }

        if !tile.window().fullscreen() {
            let new_idx = match CONFIG.general.insert_window_strategy {
                InsertWindowStrategy::EndOfSlaveStack => {
                    self.tiles.push(tile);
                    self.tiles.len() - 1
                }
                InsertWindowStrategy::ReplaceMaster => {
                    self.tiles.insert(0, tile);
                    0
                }
                InsertWindowStrategy::AfterFocused => {
                    let new_focused_idx = self.focused_tile_idx + 1;
                    if new_focused_idx == self.tiles.len() {
                        // Dont wrap around if we are on the last window, to avoid cyclic confusion.
                        self.tiles.push(tile);
                        self.tiles.len() - 1
                    } else {
                        self.tiles.insert(new_focused_idx, tile);
                        new_focused_idx
                    }
                }
            };

            if CONFIG.general.focus_new_windows {
                self.focused_tile_idx = new_idx;
            }
        } else {
            self.fullscreen = Some(FullscreenTile {
                inner: tile,
                last_known_idx: self.tiles.len(),
            });
        }

        self.refresh();
        self.arrange_tiles(animate);
        // Stop location animation, the tile should spawn "in-place"
        self.tile_mut_for(&window)
            .unwrap()
            .stop_location_animation();
    }

    pub fn remove_tile(&mut self, window: &Window, animate: bool) -> Option<Tile> {
        if self
            .fullscreen
            .as_ref()
            .is_some_and(|fs| fs.inner.window() == window)
        {
            let FullscreenTile { inner, .. } = self.take_fullscreen().unwrap();
            self.arrange_tiles(animate);
            return Some(inner);
        }

        let Some(idx) = self.tiles.iter().position(|tile| tile.window() == window) else {
            return None;
        };

        let tile = self.tiles.remove(idx);
        // "Un"-configure the window (for potentially inserting it on another workspace who knows)
        tile.window().leave_output(&self.output);
        self.focused_tile_idx = self
            .focused_tile_idx
            .clamp(0, self.tiles.len().saturating_sub(1));

        self.arrange_tiles(animate);
        Some(tile)
    }

    pub fn take_fullscreen(&mut self) -> Option<FullscreenTile> {
        self.fullscreen.take().map(|mut fs| {
            fs.inner.window().leave_output(&self.output);
            fs.inner.window().request_fullscreen(false);
            fs.inner.send_pending_configure();

            fs
        })
    }
}

// window focus
impl Workspace {
    pub fn focused(&self) -> Option<Window> {
        if let Some(fullscreen) = self.fullscreen.as_ref() {
            return Some(fullscreen.inner.window().clone());
        }

        self.tiles
            .get(self.focused_tile_idx)
            .map(Tile::window)
            .cloned()
    }

    pub fn fullscreen_window(&mut self, window: &Window, animate: bool) {
        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
        }

        let Some(idx) = self.tiles.iter().position(|t| t.window() == window) else {
            return;
        };
        let tile = self.remove_tile(window, true).unwrap();
        tile.window().request_fullscreen(true);
        // redo the configuration that remove_tile() did
        tile.window()
            .request_bounds(Some(self.output.geometry().size));
        self.fullscreen = Some(FullscreenTile {
            inner: tile,
            last_known_idx: idx,
        });
        self.refresh();
        self.arrange_tiles(animate);
    }

    pub fn focused_tile(&self) -> Option<&Tile> {
        if let Some(fullscreen) = self.fullscreen.as_ref() {
            return Some(&fullscreen.inner);
        }
        self.tiles.get(self.focused_tile_idx)
    }

    pub fn focused_tile_mut(&mut self) -> Option<&mut Tile> {
        if let Some(fullscreen) = self.fullscreen.as_mut() {
            return Some(&mut fullscreen.inner);
        }
        self.tiles.get_mut(self.focused_tile_idx)
    }

    pub fn focus_window(&mut self, window: &Window, animate: bool) {
        if let Some(idx) = self.tiles.iter().position(|tile| tile.window() == window) {
            if let Some(FullscreenTile {
                inner,
                last_known_idx,
            }) = self.take_fullscreen()
            {
                self.tiles.insert(last_known_idx, inner);
                self.arrange_tiles(animate);
            }

            self.focused_tile_idx = idx;

            self.refresh();
        }
    }

    pub fn focus_next_window(&mut self, animate: bool) -> Option<Window> {
        if self.tiles.is_empty() {
            return None;
        }

        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
            self.refresh();
            self.arrange_tiles(animate);
        }

        let tiles_len = self.tiles.len();
        let new_focused_idx = self.focused_tile_idx + 1;
        self.focused_tile_idx = if new_focused_idx == tiles_len {
            0
        } else {
            new_focused_idx
        };

        let tile = &self.tiles[self.focused_tile_idx];
        Some(tile.window().clone())
    }

    pub fn focus_previous_window(&mut self, animate: bool) -> Option<Window> {
        if self.tiles.is_empty() {
            return None;
        }

        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
            self.refresh();
            self.arrange_tiles(animate);
        }

        let windows_len = self.tiles.len();
        self.focused_tile_idx = match self.focused_tile_idx.checked_sub(1) {
            Some(idx) => idx,
            None => windows_len - 1,
        };

        let tile = &self.tiles[self.focused_tile_idx];
        Some(tile.window().clone())
    }
}

// window swapping
impl Workspace {
    pub fn swap_windows(&mut self, a: &Window, b: &Window, animate: bool) {
        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
        }

        let Some(a_idx) = self.tiles.iter().position(|tile| tile.window() == a) else {
            return;
        };
        let Some(b_idx) = self.tiles.iter().position(|tile| tile.window() == b) else {
            return;
        };
        self.focused_tile_idx = b_idx;
        self.tiles.swap(a_idx, b_idx);
        self.arrange_tiles(animate);
    }

    pub fn swap_with_next_window(&mut self, animate: bool) {
        if self.tiles.len() < 2 {
            return;
        }

        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
            self.refresh();
        }

        let tiles_len = self.tiles.len();
        let last_focused_idx = self.focused_tile_idx;

        let new_focused_idx = self.focused_tile_idx + 1;
        let new_focused_idx = if new_focused_idx == tiles_len {
            0
        } else {
            new_focused_idx
        };

        self.focused_tile_idx = new_focused_idx;
        self.tiles.swap(last_focused_idx, new_focused_idx);
        self.arrange_tiles(animate);
    }

    pub fn swap_with_previous_window(&mut self, animate: bool) {
        if self.tiles.len() < 2 {
            return;
        }

        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
            self.refresh();
        }

        let tiles_len = self.tiles.len();
        let last_focused_idx = self.focused_tile_idx;

        let new_focused_idx = match self.focused_tile_idx.checked_sub(1) {
            Some(idx) => idx,
            None => tiles_len - 1,
        };

        self.focused_tile_idx = new_focused_idx;
        self.tiles.swap(last_focused_idx, new_focused_idx);
        self.arrange_tiles(animate);
    }
}

// Geometry and layout
impl Workspace {
    pub fn window_geometry(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        self.tile_for(window).map(Tile::window_geometry)
    }

    pub fn window_visual_geometry(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        self.tile_for(window).map(Tile::window_visual_geometry)
    }

    pub fn prepare_window_geometry(&mut self, window: Window, border_config: Option<BorderConfig>) {
        let mut tile = Tile::new(window, border_config);

        if tile.window().maximized() {
            let usable_geo = self.layout.usable_geo();
            tile.window().request_size(usable_geo.size);
            return;
        }

        if tile.window().fullscreen() {
            let output_size = self.output.geometry().size;
            tile.window().request_size(output_size);
            return;
        }

        // Code adapted from arrange_tiles
        // We only care about the non-maximized and non-fullscreen tiles here
        let tiled = self
            .tiles
            .iter_mut()
            .filter(|tile| !tile.window().maximized() && !tile.is_closing());
        self.layout
            .arrange_tiles(tiled.chain(std::iter::once(&mut tile)), true);
        // The tile will just drop out from here.
        // It didnt matter much anyway, only as an intermediary to compute window size
    }

    #[profiling::function]
    pub fn arrange_tiles(&mut self, animate: bool) {
        if let Some(FullscreenTile { inner, .. }) = self.fullscreen.as_mut() {
            // NOTE: Output top left is always (0,0) locally
            let mut output_geo = self.output.geometry();
            output_geo.loc = (0, 0).into();
            inner.set_geometry(output_geo, animate);
        }

        if self.tiles.is_empty() {
            return;
        }

        let (maximized, tiled) = self
            .tiles
            .iter_mut()
            .filter(|tile| !tile.is_closing())
            .partition::<Vec<_>, _>(|tile| tile.window().maximized());

        let maximized_geo = self.layout.usable_geo();
        for tile in maximized {
            tile.set_geometry(maximized_geo, animate)
        }

        if tiled.is_empty() {
            return;
        }

        self.layout.arrange_tiles(tiled.into_iter(), animate);
    }

    pub fn select_next_layout(&mut self, animate: bool) {
        self.layout.select_next();
        self.arrange_tiles(animate);
    }

    pub fn select_previous_layout(&mut self, animate: bool) {
        self.layout.select_previous();
        self.arrange_tiles(animate);
    }

    pub fn change_mwfact(&mut self, delta: f32, animate: bool) {
        self.layout.change_mwfact(delta);
        self.arrange_tiles(animate);
    }

    pub fn change_nmaster(&mut self, delta: i32, animate: bool) {
        self.layout.change_nmaster(delta);
        self.arrange_tiles(animate);
    }

    pub fn output_resized(&mut self) {
        self.layout.output_resized(&self.output);
        self.arrange_tiles(true);
        // force update output overlaps for all the tiles.
        self.refresh();
    }
}

// Finding windows
impl Workspace {
    pub fn find_tile(&self, surface: &WlSurface) -> Option<&Tile> {
        self.fullscreen
            .as_ref()
            .filter(|fs| fs.inner.has_surface(surface, WindowSurfaceType::ALL))
            .map(|fs| &fs.inner)
            .or_else(|| {
                self.tiles
                    .iter()
                    .find(|tile| tile.has_surface(surface, WindowSurfaceType::ALL))
            })
    }

    pub fn find_tile_mut(&mut self, surface: &WlSurface) -> Option<&mut Tile> {
        self.fullscreen
            .as_mut()
            .filter(|fs| fs.inner.has_surface(surface, WindowSurfaceType::ALL))
            .map(|fs| &mut fs.inner)
            .or_else(|| {
                self.tiles
                    .iter_mut()
                    .find(|tile| tile.has_surface(surface, WindowSurfaceType::ALL))
            })
    }

    pub fn tile_for(&self, window: &Window) -> Option<&Tile> {
        self.fullscreen
            .as_ref()
            .filter(|fs| fs.inner.window() == window)
            .map(|fs| &fs.inner)
            .or_else(|| self.tiles.iter().find(|tile| tile.window() == window))
    }

    pub fn tile_mut_for(&mut self, window: &Window) -> Option<&mut Tile> {
        self.fullscreen
            .as_mut()
            .filter(|fs| fs.inner.window() == window)
            .map(|fs| &mut fs.inner)
            .or_else(|| self.tiles.iter_mut().find(|tile| tile.window() == window))
    }

    pub fn has_window(&self, window: &Window) -> bool {
        let mut ret = false;
        ret |= self
            .fullscreen
            .as_ref()
            .is_some_and(|fs| fs.inner.window() == window);
        ret |= self.tiles.iter().any(|tile| tile.window() == window);
        ret
    }

    pub fn has_surface(&self, surface: &WlSurface) -> bool {
        let mut ret = false;
        ret |= self
            .fullscreen
            .as_ref()
            .is_some_and(|fs| fs.inner.has_surface(surface, WindowSurfaceType::ALL));
        ret |= self
            .tiles
            .iter()
            .any(|tile| tile.has_surface(surface, WindowSurfaceType::ALL));
        ret
    }

    #[profiling::function]
    pub fn window_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(Window, Point<i32, Logical>)> {
        if let Some(FullscreenTile { inner: tile, .. }) = self.fullscreen.as_ref() {
            let render_location = tile.render_location();
            if tile.window_bbox().to_f64().contains(point)
                && tile
                    .window()
                    .surface_under(point - render_location.to_f64(), WindowSurfaceType::ALL)
                    .is_some()
            {
                return Some((tile.window().clone(), render_location));
            }
        }

        if let Some(tile) = self.focused_tile() {
            let render_location = tile.render_location();
            if tile.window_bbox().to_f64().contains(point)
                && tile
                    .window()
                    .surface_under(point - render_location.to_f64(), WindowSurfaceType::ALL)
                    .is_some()
            {
                return Some((tile.window().clone(), render_location));
            }
        }

        self.tiles
            .iter()
            .filter(|tile| tile.window_bbox().to_f64().contains(point))
            .find_map(|tile| {
                let render_location = tile.render_location();
                if tile
                    .window()
                    .surface_under(point - render_location.to_f64(), WindowSurfaceType::ALL)
                    .is_some()
                {
                    Some((tile.window().clone(), render_location))
                } else {
                    None
                }
            })
    }

    #[profiling::function]
    pub fn tiles_under(&self, point: Point<f64, Logical>) -> impl Iterator<Item = &Tile> {
        self.fullscreen
            .as_ref()
            .map(|fs| &fs.inner)
            .into_iter()
            .chain(self.tiles.iter().filter(move |tile| {
                if !tile.window_bbox().to_f64().contains(point) {
                    return false;
                }

                let render_location = tile.render_location();
                tile.window()
                    .surface_under(point - render_location.to_f64(), WindowSurfaceType::ALL)
                    .is_some()
            }))
    }
}

pub struct FullscreenTile {
    pub inner: Tile,
    pub last_known_idx: usize,
}

impl PartialEq for FullscreenTile {
    fn eq(&self, other: &Self) -> bool {
        &self.inner == &other.inner
    }
}

impl ToString for WorkspaceLayout {
    fn to_string(&self) -> String {
        match self {
            Self::Tile { .. } => "tile".into(),
            Self::BottomStack { .. } => "bstack".into(),
            Self::CenteredMaster { .. } => "cmaster".into(),
            Self::Floating => "floating".into(),
        }
    }
}
