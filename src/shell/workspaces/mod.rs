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

use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Monotonic, Physical, Point, Rectangle, Scale, Time};

pub use self::layout::WorkspaceLayout;
use self::tile::{WorkspaceElement, WorkspaceTile, WorkspaceTileRenderElement};
use crate::config::{
    BorderConfig, InsertWindowStrategy, WorkspaceSwitchAnimationDirection, CONFIG,
};
use crate::fht_render_elements;
use crate::renderer::FhtRenderer;
use crate::utils::animation::Animation;
use crate::utils::output::OutputExt;

pub struct WorkspaceSet<E: WorkspaceElement> {
    output: Output,
    workspaces: Vec<Workspace<E>>,
    switch_animation: Option<WorkspaceSwitchAnimation>,
    active_idx: usize,
}

#[allow(dead_code)]
impl<E: WorkspaceElement> WorkspaceSet<E> {
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
        let layouts = CONFIG.general.layouts.clone();
        for workspace in &mut self.workspaces {
            workspace.layouts = layouts.clone();
            workspace.active_layout_idx = workspace
                .active_layout_idx
                .clamp(0, workspace.layouts.len() - 1);
        }
    }

    pub fn set_active_idx(&mut self, target_idx: usize, animate: bool) -> Option<E> {
        let target_idx = target_idx.clamp(0, 9);
        if !animate {
            self.active_idx = target_idx;
            return self.workspaces[target_idx].focused().cloned();
        }

        let active_idx = self.active_idx;
        if target_idx == active_idx || self.switch_animation.is_some() {
            return None;
        }

        self.switch_animation = Some(WorkspaceSwitchAnimation::new(target_idx));
        self.workspaces[target_idx].focused().cloned()
    }

    pub fn get_active_idx(&self) -> usize {
        if let Some(WorkspaceSwitchAnimation { target_idx, .. }) = self.switch_animation.as_ref() {
            *target_idx
        } else {
            self.active_idx
        }
    }

    pub fn drain_workspaces(&mut self) -> impl Iterator<Item = Workspace<E>> + '_ {
        self.workspaces.drain(..)
    }

    pub fn get_workspace(&self, idx: usize) -> &Workspace<E> {
        &self.workspaces[idx]
    }

    pub fn get_workspace_mut(&mut self, idx: usize) -> &mut Workspace<E> {
        &mut self.workspaces[idx]
    }

    pub fn active(&self) -> &Workspace<E> {
        if let Some(WorkspaceSwitchAnimation { target_idx, .. }) = self.switch_animation.as_ref() {
            &self.workspaces[*target_idx]
        } else {
            &self.workspaces[self.active_idx]
        }
    }

    pub fn active_mut(&mut self) -> &mut Workspace<E> {
        if let Some(WorkspaceSwitchAnimation { target_idx, .. }) = self.switch_animation.as_ref() {
            &mut self.workspaces[*target_idx]
        } else {
            &mut self.workspaces[self.active_idx]
        }
    }

    pub fn workspaces(&self) -> impl Iterator<Item = &Workspace<E>> {
        self.workspaces.iter()
    }

    pub fn workspaces_mut(&mut self) -> impl Iterator<Item = &mut Workspace<E>> {
        self.workspaces.iter_mut()
    }

    pub fn arrange(&mut self) {
        self.workspaces_mut().for_each(|ws| ws.arrange_tiles(true))
    }

    pub fn find_element(&self, surface: &WlSurface) -> Option<&E> {
        self.workspaces()
            .find_map(|ws| ws.find_tile(surface).map(WorkspaceTile::element))
    }

    pub fn find_tile_mut(&mut self, surface: &WlSurface) -> Option<&mut WorkspaceTile<E>> {
        self.workspaces_mut()
            .find_map(|ws| ws.find_tile_mut(surface))
    }

    pub fn find_workspace(&self, surface: &WlSurface) -> Option<&Workspace<E>> {
        self.workspaces().find(|ws| ws.has_surface(surface))
    }

    pub fn find_workspace_mut(&mut self, surface: &WlSurface) -> Option<&mut Workspace<E>> {
        self.workspaces_mut().find(|ws| ws.has_surface(surface))
    }

    pub fn find_element_and_workspace(&self, surface: &WlSurface) -> Option<(E, &Workspace<E>)> {
        self.workspaces().find_map(|ws| {
            let element = ws.find_tile(surface).map(|w| w.element.clone())?;
            Some((element, ws))
        })
    }

    pub fn find_element_and_workspace_mut(
        &mut self,
        surface: &WlSurface,
    ) -> Option<(E, &mut Workspace<E>)> {
        self.workspaces_mut().find_map(|ws| {
            let element = ws.find_tile(surface).map(|w| w.element.clone())?;
            Some((element, ws))
        })
    }

    pub fn visible_elements(&self) -> impl Iterator<Item = &E> + '_ {
        let switching_windows = self
            .switch_animation
            .as_ref()
            .map(|anim| {
                let ws = &self.workspaces[anim.target_idx];

                ws.fullscreen
                    .as_ref()
                    .map(|fs| fs.inner.element())
                    .into_iter()
                    .chain(ws.tiles.iter().map(WorkspaceTile::element))
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .flatten();

        let active = self.active();
        active
            .fullscreen
            .as_ref()
            .map(|fs| fs.inner.element())
            .into_iter()
            .chain(active.tiles.iter().map(WorkspaceTile::element))
            .chain(switching_windows)

    }

    pub fn ws_for(&self, element: &E) -> Option<&Workspace<E>> {
        self.workspaces().find(|ws| ws.has_element(element))
    }

    pub fn ws_mut_for(&mut self, element: &E) -> Option<&mut Workspace<E>> {
        self.workspaces_mut().find(|ws| ws.has_element(element))
    }

    #[profiling::function]
    pub fn current_fullscreen(&self) -> Option<(&E, Point<i32, Logical>)> {
        let Some(animation) = self.switch_animation.as_ref() else {
            return self.active().fullscreen.as_ref().map(|fs| {
                // Fullscreen is always at (0,0)
                (fs.inner.element(), (0, 0).into())
            });
        };

        let output_geo = self.output.geometry();
        let (current_offset, target_offset) = animation.calculate_offsets(self.active_idx, output_geo);
        self.active()
            .fullscreen
            .as_ref()
            .map(|fs| (fs.inner.element(), current_offset))
            .or_else(|| {
                self.workspaces[animation.target_idx]
                    .fullscreen
                    .as_ref()
                    .map(|fs| (fs.inner.element(), target_offset))
            })
    }

    #[profiling::function]
    pub fn element_under(&self, point: Point<f64, Logical>) -> Option<(&E, Point<i32, Logical>)> {
        let Some(animation) = self.switch_animation.as_ref() else {
            // It's just the active one, so no need to do additional calculations.
            return self.active().element_under(point);
        };

        let output_geo = self.output.geometry();
        let (current_offset, target_offset) = animation.calculate_offsets(self.active_idx, output_geo);

        self.active()
            .element_under(point + current_offset.to_f64())
            .map(|(ft, loc)| (ft, loc + current_offset))
            .or_else(|| {
                self.workspaces[animation.target_idx]
                    .element_under(point + target_offset.to_f64())
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
            elements =
                target_elements
                    .map(WorkspaceSetRenderElement::Normal)
                    .collect()
            );
            return (target.fullscreen.is_some(), elements);
        }

        // Otherwise to computations
        let (current_offset, target_offset) = animation.calculate_offsets(self.active_idx, output_geo);

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

pub struct Workspace<E: WorkspaceElement> {
    pub output: Output,
    pub tiles: Vec<WorkspaceTile<E>>,
    focused_tile_idx: usize,
    pub layouts: Vec<WorkspaceLayout>,
    pub fullscreen: Option<FullscreenTile<E>>,
    pub active_layout_idx: usize,
    id: WorkspaceId,
}

impl<E: WorkspaceElement> Workspace<E> {
    pub fn new(output: Output) -> Self {
        Self {
            output,
            tiles: vec![],
            focused_tile_idx: 0,
            layouts: CONFIG.general.layouts.clone(),
            active_layout_idx: 0,
            fullscreen: None,
            id: WorkspaceId::unique(),
        }
    }

    pub fn id(&self) -> WorkspaceId {
        self.id
    }

    pub fn tiles(&self) -> impl Iterator<Item = &WorkspaceTile<E>> {
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
            .is_some_and(|fs| !fs.inner.element.fullscreen())
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
            fullscreen.inner.element.set_activated(true);

            let mut bbox = fullscreen.inner.element.bbox();
            bbox.loc = fullscreen.inner.location + output_geometry.loc;
            if let Some(mut overlap) = output_geometry.intersection(bbox) {
                // output_enter excepts the overlap to be relative to the element, weird choice
                // but I comply.
                overlap.loc -= bbox.loc;
                fullscreen.inner.element.output_enter(&self.output, overlap);
            }

            fullscreen.inner.send_pending_configure();
            fullscreen.inner.element.refresh();
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
            tile.element.set_activated(idx == self.focused_tile_idx);

            let mut bbox = tile.element.bbox();
            bbox.loc = tile.location + output_geometry.loc;

            if let Some(mut overlap) = output_geometry.intersection(bbox) {
                // output_enter excepts the overlap to be relative to the element, weird choice
                // but I comply.
                overlap.loc -= bbox.loc;
                tile.element.output_enter(&self.output, overlap);
            }

            tile.send_pending_configure();
            tile.element.refresh();
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
impl<E: WorkspaceElement> Workspace<E> {
    pub fn insert_tile(&mut self, tile: WorkspaceTile<E>, animate: bool) {
        let WorkspaceTile {
            element,
            border_config,
            ..
        } = tile;
        self.insert_element(element, border_config, animate);
    }

    pub fn insert_element(
        &mut self,
        element: E,
        border_config: Option<BorderConfig>,
        animate: bool,
    ) {
        if self.has_element(&element) {
            return;
        }

        // Output overlap + wl_surface scale and transform will be set when using self.refresh
        element.set_bounds(Some(self.output.geometry().size));
        let tile = WorkspaceTile::new(element, border_config);

        // NOTE: In the following code we dont call to send_pending_configure since arrange_tiles
        // does this for us automatically.

        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
        }

        if !tile.element.fullscreen() {
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
    }

    pub fn remove_tile(&mut self, element: &E, animate: bool) -> Option<WorkspaceTile<E>> {
        if self
            .fullscreen
            .as_ref()
            .is_some_and(|fs| fs.inner == *element)
        {
            let FullscreenTile { inner, .. } = self.take_fullscreen().unwrap();
            self.arrange_tiles(animate);

            return Some(inner);
        }

        let Some(idx) = self.tiles.iter().position(|t| t.element == *element) else {
            return None;
        };

        let tile = self.tiles.remove(idx);
        // "Un"-configure the window (for potentially inserting it on another workspace who knows)
        tile.element.output_leave(&self.output);
        self.focused_tile_idx = self
            .focused_tile_idx
            .clamp(0, self.tiles.len().saturating_sub(1));

        self.arrange_tiles(animate);
        Some(tile)
    }

    pub fn take_fullscreen(&mut self) -> Option<FullscreenTile<E>> {
        self.fullscreen.take().map(|mut fs| {
            fs.inner.element.output_leave(&self.output);
            fs.inner.element.set_fullscreen(false);
            fs.inner.element.set_fullscreen_output(None);
            fs.inner.send_pending_configure();

            fs
        })
    }
}

// Element focus
impl<E: WorkspaceElement> Workspace<E> {
    pub fn focused(&self) -> Option<&E> {
        if let Some(fullscreen) = self.fullscreen.as_ref() {
            return Some(&fullscreen.inner.element);
        }

        self.tiles
            .get(self.focused_tile_idx)
            .map(WorkspaceTile::element)
    }

    pub fn fullscreen_element(&mut self, element: &E, animate: bool) {
        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
        }

        let Some(idx) = self.tiles.iter().position(|t| t == element) else {
            return;
        };
        let tile = self.remove_tile(element, true).unwrap();
        tile.element.set_fullscreen(true);
        // redo the configuration that remove_tile() did
        tile.element.set_bounds(Some(self.output.geometry().size));
        self.fullscreen = Some(FullscreenTile {
            inner: tile,
            last_known_idx: idx,
        });
        self.refresh();
        self.arrange_tiles(animate);
    }

    pub fn focused_tile(&self) -> Option<&WorkspaceTile<E>> {
        if let Some(fullscreen) = self.fullscreen.as_ref() {
            return Some(&fullscreen.inner);
        }
        self.tiles.get(self.focused_tile_idx)
    }

    pub fn focused_tile_mut(&mut self) -> Option<&mut WorkspaceTile<E>> {
        if let Some(fullscreen) = self.fullscreen.as_mut() {
            return Some(&mut fullscreen.inner);
        }
        self.tiles.get_mut(self.focused_tile_idx)
    }

    pub fn focus_element(&mut self, window: &E, animate: bool) {
        if let Some(idx) = self.tiles.iter().position(|w| w == window) {
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

    pub fn focus_next_element(&mut self, animate: bool) -> Option<&E> {
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
        Some(tile.element())
    }

    pub fn focus_previous_element(&mut self, animate: bool) -> Option<&E> {
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
        Some(tile.element())
    }
}

// Element swapping
impl<E: WorkspaceElement> Workspace<E> {
    pub fn swap_elements(&mut self, a: &E, b: &E, animate: bool) {
        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
        }

        let Some(a_idx) = self.tiles.iter().position(|tile| tile.element == *a) else {
            return;
        };
        let Some(b_idx) = self.tiles.iter().position(|tile| tile.element == *b) else {
            return;
        };
        self.focused_tile_idx = b_idx;
        self.tiles.swap(a_idx, b_idx);
        self.arrange_tiles(animate);
    }

    pub fn swap_with_next_element(&mut self, animate: bool) {
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

    pub fn swap_with_previous_element(&mut self, animate: bool) {
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
impl<E: WorkspaceElement> Workspace<E> {
    pub fn element_geometry(&self, element: &E) -> Option<Rectangle<i32, Logical>> {
        self.tile_for(element).map(WorkspaceTile::element_geometry)
    }

    pub fn element_visual_geometry(&self, element: &E) -> Option<Rectangle<i32, Logical>> {
        self.tile_for(element)
            .map(WorkspaceTile::element_visual_geometry)
    }

    pub fn tile_area(&self) -> Rectangle<i32, Logical> {
        let mut area = layer_map_for_output(&self.output).non_exclusive_zone();
        let outer_gaps = CONFIG.general.outer_gaps;
        area.size -= (2 * outer_gaps, 2 * outer_gaps).into();
        area.loc += (outer_gaps, outer_gaps).into();
        area
    }

    #[profiling::function]
    pub fn arrange_tiles(&mut self, animate: bool) {
        if let Some(FullscreenTile { inner, .. }) = self.fullscreen.as_mut() {
            // NOTE: Output top left is always (0,0) locally
            let mut output_geo = self.output.geometry();
            output_geo.loc = (0, 0).into();
            inner.set_tile_geometry(output_geo, animate);
        }

        if self.tiles.is_empty() {
            return;
        }

        let inner_gaps = CONFIG.general.inner_gaps;
        let tile_area = self.tile_area();

        let layout = self.get_active_layout();
        let (maximized, tiled) = self
            .tiles
            .iter_mut()
            .filter(|tile| {
                // Do not include tiles that are closing.
                !matches!(
                    tile.open_close_animation,
                    Some(tile::OpenCloseAnimation::Closing { .. })
                )
            })
            .partition::<Vec<_>, _>(|tile| tile.element.maximized());

        for tile in maximized {
            tile.set_tile_geometry(tile_area, animate)
        }

        if tiled.is_empty() {
            return;
        }

        layout.arrange_tiles(tiled.into_iter(), tile_area, inner_gaps, animate);
    }

    pub fn get_active_layout(&self) -> WorkspaceLayout {
        self.layouts[self.active_layout_idx]
    }

    pub fn select_next_layout(&mut self, animate: bool) {
        let layouts_len = self.layouts.len();
        let new_active_idx = self.active_layout_idx + 1;
        let new_active_idx = if new_active_idx == layouts_len {
            0
        } else {
            new_active_idx
        };

        self.active_layout_idx = new_active_idx;
        self.arrange_tiles(animate);
    }

    pub fn select_previous_layout(&mut self, animate: bool) {
        let layouts_len = self.layouts.len();
        let new_active_idx = match self.active_layout_idx.checked_sub(1) {
            Some(idx) => idx,
            None => layouts_len - 1,
        };

        self.active_layout_idx = new_active_idx;
        self.arrange_tiles(animate);
    }

    pub fn change_mwfact(&mut self, delta: f32, animate: bool) {
        let active_layout = &mut self.layouts[self.active_layout_idx];
        if let WorkspaceLayout::Tile {
            master_width_factor,
            ..
        }
        | WorkspaceLayout::BottomStack {
            master_width_factor,
            ..
        }
        | WorkspaceLayout::CenteredMaster {
            master_width_factor,
            ..
        } = active_layout
        {
            *master_width_factor += delta;
            *master_width_factor = master_width_factor.clamp(0.05, 0.95);
        }
        self.arrange_tiles(animate);
    }

    pub fn change_nmaster(&mut self, delta: i32, animate: bool) {
        let active_layout = &mut self.layouts[self.active_layout_idx];
        if let WorkspaceLayout::Tile { nmaster, .. }
        | WorkspaceLayout::BottomStack { nmaster, .. }
        | WorkspaceLayout::CenteredMaster { nmaster, .. } = active_layout
        {
            let new_nmaster = nmaster
                .saturating_add_signed(delta as isize)
                .clamp(1, usize::MAX);
            *nmaster = new_nmaster;
        }
        self.arrange_tiles(animate);
    }
}

// Finding elements
impl<E: WorkspaceElement> Workspace<E> {
    pub fn find_tile(&self, surface: &WlSurface) -> Option<&WorkspaceTile<E>> {
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

    pub fn find_tile_mut(&mut self, surface: &WlSurface) -> Option<&mut WorkspaceTile<E>> {
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

    pub fn tile_for(&self, element: &E) -> Option<&WorkspaceTile<E>> {
        self.fullscreen
            .as_ref()
            .filter(|fs| fs.inner == *element)
            .map(|fs| &fs.inner)
            .or_else(|| self.tiles.iter().find(|tile| *tile == element))
    }

    pub fn tile_mut_for(&mut self, element: &E) -> Option<&mut WorkspaceTile<E>> {
        self.fullscreen
            .as_mut()
            .filter(|fs| fs.inner == *element)
            .map(|fs| &mut fs.inner)
            .or_else(|| self.tiles.iter_mut().find(|tile| *tile == element))
    }

    pub fn has_element(&self, element: &E) -> bool {
        let mut ret = false;
        ret |= self
            .fullscreen
            .as_ref()
            .is_some_and(|fs| fs.inner == *element);
        ret |= self.tiles.iter().any(|tile| tile == element);
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
    pub fn element_under(&self, point: Point<f64, Logical>) -> Option<(&E, Point<i32, Logical>)> {
        if let Some(FullscreenTile { inner: tile, .. }) = self.fullscreen.as_ref() {
            let render_location = tile.render_location();
            if tile.bbox().to_f64().contains(point)
                && tile
                    .element
                    .is_in_input_region(&(point - render_location.to_f64()))
            {
                return Some((tile.element(), render_location));
            }
        }

        if let Some(tile) = self.focused_tile() {
            let render_location = tile.render_location();
            if tile.bbox().to_f64().contains(point)
                && tile
                    .element
                    .is_in_input_region(&(point - render_location.to_f64()))
            {
                return Some((tile.element(), render_location));
            }
        }

        self.tiles
            .iter()
            .filter(|tile| tile.bbox().to_f64().contains(point))
            .find_map(|tile| {
                let render_location = tile.render_location();
                if tile
                    .element
                    .is_in_input_region(&(point - render_location.to_f64()))
                {
                    Some((tile.element(), render_location))
                } else {
                    None
                }
            })
    }

    #[profiling::function]
    pub fn tiles_under(
        &self,
        point: Point<f64, Logical>,
    ) -> impl Iterator<Item = &WorkspaceTile<E>> {
        self.fullscreen
            .as_ref()
            .map(|fs| &fs.inner)
            .into_iter()
            .chain(self.tiles.iter().filter(move |tile| {
                if !tile.bbox().to_f64().contains(point) {
                    return false;
                }

                let render_location = tile.render_location();
                tile.element
                    .is_in_input_region(&(point - render_location.to_f64()))
            }))
    }
}

pub struct FullscreenTile<E: WorkspaceElement> {
    pub inner: WorkspaceTile<E>,
    pub last_known_idx: usize,
}

impl<E: WorkspaceElement> PartialEq for FullscreenTile<E> {
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

#[cfg(test)]
mod tests {
    // How stuff is tested here is very similar to Niri's layout tests, with operations that we
    // apply to workspaces, then we check some invariants.
    use std::borrow::Cow;
    use std::cell::RefCell;
    use std::rc::Rc;

    use smithay::desktop::space::SpaceElement;
    use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
    use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
    use smithay::utils::{IsAlive, Logical, Point, Rectangle, Size};
    use smithay::wayland::seat::WaylandFocus;

    use super::tile::WorkspaceElement;
    use super::{Workspace, WorkspaceLayout};
    use crate::config::{BorderConfig, CONFIG};
    use crate::utils::output::OutputExt;

    struct TestElement(Rc<RefCell<TestElementInner>>);
    impl std::fmt::Debug for TestElement {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            std::fmt::Debug::fmt(&self.0, f)
        }
    }
    impl Clone for TestElement {
        fn clone(&self) -> Self {
            Self(Rc::clone(&self.0))
        }
    }

    impl TestElement {
        fn new(bbox: Rectangle<i32, Logical>) -> Self {
            let inner = TestElementInner {
                bbox,
                requested_size: None,
                bounds: None,
                outputs: vec![],
                fullscreen: false,
                maximized: false,
                activated: false,
                alive: true,
            };

            Self(Rc::new(RefCell::new(inner)))
        }

        fn output_entered(&self, output: &Output) -> bool {
            let guard = self.0.borrow();
            guard.outputs.iter().any(|o| o == output)
        }
    }

    #[derive(Debug)]
    struct TestElementInner {
        bbox: Rectangle<i32, Logical>,
        requested_size: Option<Size<i32, Logical>>,
        bounds: Option<Size<i32, Logical>>,
        outputs: Vec<Output>,
        fullscreen: bool,
        maximized: bool,
        activated: bool,
        alive: bool,
    }

    impl SpaceElement for TestElement {
        fn bbox(&self) -> Rectangle<i32, Logical> {
            self.0.borrow().bbox
        }

        fn is_in_input_region(&self, point: &smithay::utils::Point<f64, Logical>) -> bool {
            // For this, the location will already be local to the bounding box.
            // So we change the bbox from global to local
            let mut bbox = self.0.borrow().bbox.to_f64();
            bbox.loc = Point::default();
            bbox.contains(*point)
        }

        fn set_activate(&self, activated: bool) {
            self.0.borrow_mut().activated = activated
        }

        fn output_enter(&self, output: &Output, _overlap: Rectangle<i32, Logical>) {
            self.0.borrow_mut().outputs.push(output.clone())
        }

        fn output_leave(&self, output: &Output) {
            self.0.borrow_mut().outputs.retain(|o| o != output)
        }
    }

    impl IsAlive for TestElement {
        fn alive(&self) -> bool {
            self.0.borrow().alive
        }
    }

    impl WaylandFocus for TestElement {
        fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
            None
        }
    }

    impl PartialEq for TestElement {
        fn eq(&self, other: &Self) -> bool {
            Rc::ptr_eq(&self.0, &other.0)
        }
    }

    impl WorkspaceElement for TestElement {
        fn send_pending_configure(&self) {
            let mut guard = self.0.borrow_mut();
            if let Some(requested_size) = guard.requested_size.take() {
                guard.bbox.size = requested_size;
            }
        }

        fn set_size(&self, new_size: Size<i32, Logical>) {
            let mut guard = self.0.borrow_mut();
            guard.requested_size = Some(new_size);
        }

        fn size(&self) -> Size<i32, Logical> {
            self.0.borrow().bbox.size
        }

        fn set_fullscreen(&self, fullscreen: bool) {
            self.0.borrow_mut().fullscreen = fullscreen
        }

        fn set_fullscreen_output(
            &self,
            output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
        ) {
            let _ = output; // no need really.
        }

        fn fullscreen(&self) -> bool {
            self.0.borrow().fullscreen
        }

        fn fullscreen_output(
            &self,
        ) -> Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput> {
            None
        }

        fn set_maximized(&self, maximize: bool) {
            self.0.borrow_mut().maximized = maximize
        }

        fn maximized(&self) -> bool {
            self.0.borrow().maximized
        }

        fn set_bounds(&self, bounds: Option<Size<i32, Logical>>) {
            self.0.borrow_mut().bounds = bounds;
        }

        fn bounds(&self) -> Option<Size<i32, Logical>> {
            self.0.borrow().bounds
        }

        fn set_activated(&self, activated: bool) {
            self.0.borrow_mut().activated = activated;
        }

        fn activated(&self) -> bool {
            self.0.borrow_mut().activated
        }

        fn app_id(&self) -> String {
            String::from("test.window")
        }

        fn title(&self) -> String {
            String::from("Test Window")
        }

        fn render_surface_elements<R: crate::renderer::FhtRenderer>(
            &self,
            _renderer: &mut R,
            _location: Point<i32, smithay::utils::Physical>,
            _scale: smithay::utils::Scale<f64>,
            _alpha: f32,
        ) -> Vec<smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<R>>
        {
            vec![]
        }

        fn render_popup_elements<R: crate::renderer::FhtRenderer>(
            &self,
            _renderer: &mut R,
            _location: Point<i32, smithay::utils::Physical>,
            _scale: smithay::utils::Scale<f64>,
            _alpha: f32,
        ) -> Vec<smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<R>>
        {
            vec![]
        }

        fn set_offscreen_element_id(&self, id: Option<smithay::backend::renderer::element::Id>) {
            let _ = id; // we are not rendering
        }

        fn get_offscreen_element_id(&self) -> Option<smithay::backend::renderer::element::Id> {
            None //  we are not rendering
        }
    }

    #[allow(unused)]
    const TILE_LAYOUT: usize = 0;
    #[allow(unused)]
    const BOTTOM_STACK_LAYOUT: usize = 1;
    const FLOATING_LAYOUT: usize = 2;

    fn create_workspace() -> Workspace<TestElement> {
        let output = Output::new(
            String::from("test-output-0"),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: String::from("test-make"),
                model: String::from("test-model"),
            },
        );
        let mode = Mode {
            size: (800, 600).into(),
            refresh: 0, // does not matter
        };
        output.add_mode(mode);
        output.set_preferred(mode);
        output.change_current_state(Some(mode), None, None, None);

        Workspace {
            output,
            tiles: vec![],
            focused_tile_idx: 0,
            layouts: vec![
                WorkspaceLayout::Tile {
                    nmaster: 1,
                    master_width_factor: 0.5,
                },
                WorkspaceLayout::BottomStack {
                    nmaster: 1,
                    master_width_factor: 0.5,
                },
                WorkspaceLayout::Floating,
            ],
            active_layout_idx: 0,
            fullscreen: None,
            id: super::WorkspaceId::unique()
        }
    }

    // TODO: Actually find a way to test layouts without having to hardcore pre computed values
    #[allow(unused)]
    enum Operation {
        InsertElement {
            element: TestElement,
            border_config: Option<BorderConfig>,
        },
        RemoveTile {
            element: TestElement,
        },
        FullscreenElement {
            element: TestElement,
        },
        FocusElement {
            element: TestElement,
        },
        FocusNextElement,
        FocusPreviousElement,
        SwapElements {
            a: TestElement,
            b: TestElement,
        },
        SwapWithNextElement,
        SwapWithPreviousElement,
        ArrangeTiles,
        SetLayout {
            layout_idx: usize,
        },
        SelectNextLayout,
        SelectPreviousLayout,
        ChangeMwfact {
            delta: f32,
        },
        ChangeNmaster {
            delta: i32,
        },
    }

    impl Operation {
        fn apply(self, workspace: &mut Workspace<TestElement>) {
            match self {
                Operation::InsertElement {
                    element,
                    border_config,
                } => workspace.insert_element(element, border_config, false),
                Operation::RemoveTile { element } => {
                    let _ = workspace.remove_tile(&element, false);
                }
                Operation::FullscreenElement { element } => {
                    workspace.fullscreen_element(&element, false)
                }
                Operation::FocusElement { element } => workspace.focus_element(&element, false),
                Operation::FocusNextElement => {
                    let _ = workspace.focus_next_element(false);
                }
                Operation::FocusPreviousElement => {
                    let _ = workspace.focus_previous_element(false);
                }
                Operation::SwapElements { a, b } => {
                    let _ = workspace.swap_elements(&a, &b, false);
                }
                Operation::SwapWithNextElement => {
                    let _ = workspace.swap_with_next_element(false);
                }
                Operation::SwapWithPreviousElement => {
                    let _ = workspace.swap_with_previous_element(false);
                }
                Operation::ArrangeTiles => workspace.arrange_tiles(false),
                Operation::SetLayout { layout_idx } => {
                    workspace.active_layout_idx = layout_idx;
                    workspace.arrange_tiles(false);
                }
                Operation::SelectNextLayout => workspace.select_next_layout(false),
                Operation::SelectPreviousLayout => workspace.select_previous_layout(false),
                Operation::ChangeMwfact { delta } => workspace.change_mwfact(delta, false),
                Operation::ChangeNmaster { delta } => workspace.change_nmaster(delta, false),
            }
        }
    }

    fn check_operations(operations: Vec<Operation>) -> Workspace<TestElement> {
        // NOTE: We have to set the configuration since it never defaults.
        // Optimally we wouldn't want a global constant here, but I digress
        CONFIG.set(Default::default());

        let mut workspace = create_workspace();
        for operation in operations {
            operation.apply(&mut workspace);
        }

        workspace.check_invariants();
        workspace
    }

    impl Workspace<TestElement> {
        fn check_invariants(&self) {
            let output_geo = self.output.geometry();

            // State checks.
            if self.tiles.len() != 0 {
                // Edge case when we dont have any tiles, we have focused_tile_idx = len = 0
                assert!(
                    self.focused_tile_idx < self.tiles.len(),
                    "Focus tile index should be strictly smaller than tiles.len()"
                );
            }
            assert!(
                !self.layouts.is_empty(),
                "A workspace can't exist without layouts!"
            );
            assert!(
                self.active_layout_idx < self.layouts.len(),
                "Active layout index should be strictly smaller than layouts.len()"
            );

            // General checks for mapped tiles that abide to the layout
            for tile in &self.tiles {
                assert!(
                    tile.element.output_entered(&self.output),
                    "Tile element should enter the workspace's output!"
                );

                assert!(
                    !tile.element.fullscreen(),
                    "Tile element should not be in fullscreen state!"
                );

                assert_eq!(
                    tile.element.bounds(),
                    Some(output_geo.size),
                    "Tile element bounds should match the output size!"
                );
            }

            if let Some(fullscreen) = self.fullscreen.as_ref() {
                assert!(
                    fullscreen.inner.element.output_entered(&self.output),
                    "Fullscreen tile element should enter the workspace's output!"
                );
                assert!(
                    fullscreen.inner.element.fullscreen(),
                    "Fullscreened tile element be in fullscreen state!"
                );
            }
        }
    }

    // The following tests are non exhaustive and more are coming sooner or later.
    // ---
    // They are meant to simulate how a user might interact with a workspace through a variety of
    // cases, with tests for invariants and expected results to ensure:
    // - Expected behaviour for the end user
    // - Proper usage of wayland protocols (especially xdg_toplevel) for the backend

    #[test]
    fn insert_element() {
        let element = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let workspace = check_operations(vec![Operation::InsertElement {
            element: element.clone(),
            border_config: None,
        }]);

        let loc = {
            let value = CONFIG.general.outer_gaps + CONFIG.decoration.border.thickness as i32;
            (value, value).into()
        };
        let size = {
            let width =
                800 - 2 * (CONFIG.general.outer_gaps + CONFIG.decoration.border.thickness as i32);
            let height =
                600 - 2 * (CONFIG.general.outer_gaps + CONFIG.decoration.border.thickness as i32);
            (width, height).into()
        };

        let tile = workspace.tile_for(&element).unwrap();
        assert_eq!(tile.location, loc);
        assert_eq!(element.bbox().size, size);
    }

    #[test]
    fn insert_element_twice() {
        let element = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let workspace = check_operations(vec![
            Operation::InsertElement {
                element: element.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: element.clone(),
                border_config: None,
            },
        ]);

        let loc = {
            let value = CONFIG.general.outer_gaps + CONFIG.decoration.border.thickness as i32;
            (value, value).into()
        };
        let size = {
            let width =
                800 - 2 * (CONFIG.general.outer_gaps + CONFIG.decoration.border.thickness as i32);
            let height =
                600 - 2 * (CONFIG.general.outer_gaps + CONFIG.decoration.border.thickness as i32);
            (width, height).into()
        };

        let tile = workspace.tile_for(&element).unwrap();
        assert_eq!(workspace.tiles.len(), 1); // we can't insert an element twice
        assert_eq!(tile.location, loc);
        assert_eq!(element.bbox().size, size);
    }

    #[test]
    fn insert_element_with_border_config() {
        let element = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let border_config = BorderConfig {
            thickness: 5,
            ..Default::default()
        };
        let workspace = check_operations(vec![Operation::InsertElement {
            element: element.clone(),
            border_config: Some(border_config),
        }]);

        let loc = {
            let value = CONFIG.general.outer_gaps + border_config.thickness as i32;
            (value, value).into()
        };
        let size = {
            let width = 800 - 2 * (CONFIG.general.outer_gaps + border_config.thickness as i32);
            let height = 600 - 2 * (CONFIG.general.outer_gaps + border_config.thickness as i32);
            (width, height).into()
        };

        let tile = workspace.tile_for(&element).unwrap();
        assert_eq!(tile.location, loc);
        assert_eq!(element.bbox().size, size);
    }

    #[test]
    fn insert_element_with_floating_layout() {
        let element = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let workspace = check_operations(vec![
            Operation::SetLayout {
                layout_idx: FLOATING_LAYOUT,
            },
            Operation::InsertElement {
                element: element.clone(),
                border_config: None,
            },
        ]);

        let tile = workspace.tile_for(&element).unwrap();
        assert_eq!(tile.location, (0, 0).into());
        assert_eq!(element.bbox().size, (200, 200).into());
    }

    #[test]
    fn insert_fullscreen_element() {
        let element = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        element.set_fullscreen(true);
        let workspace = check_operations(vec![Operation::InsertElement {
            element: element.clone(),
            border_config: None,
        }]);

        assert!(workspace.fullscreen.is_some());
        assert!(element.fullscreen());
    }

    #[test]
    fn remove_element() {
        let element = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let workspace = check_operations(vec![
            Operation::InsertElement {
                element: element.clone(),
                border_config: None,
            },
            Operation::RemoveTile {
                element: element.clone(),
            },
        ]);

        assert_eq!(workspace.tiles.len(), 0);
    }

    #[test]
    fn remove_fullscreen_element() {
        let element = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        element.set_fullscreen(true);
        let workspace = check_operations(vec![
            Operation::InsertElement {
                element: element.clone(),
                border_config: None,
            },
            Operation::RemoveTile {
                element: element.clone(),
            },
        ]);

        assert_eq!(workspace.tiles.len(), 0);
        assert!(workspace.fullscreen.is_none());
    }

    #[test]
    fn fullscreen_element() {
        let element = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let workspace = check_operations(vec![
            Operation::InsertElement {
                element: element.clone(),
                border_config: None,
            },
            Operation::FullscreenElement {
                element: element.clone(),
            },
        ]);

        assert!(element.fullscreen());
        assert!(workspace.fullscreen.is_some());
    }

    #[test]
    fn focus_element() {
        let a = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let b = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let c = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));

        let workspace = check_operations(vec![
            Operation::InsertElement {
                element: a.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: b.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: c.clone(),
                border_config: None,
            },
            Operation::FocusElement { element: b.clone() },
        ]);

        assert_eq!(workspace.focused_tile_idx, 1);
    }

    #[test]
    fn focus_element_removes_fullscreen() {
        let a = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let b = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let c = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        c.set_fullscreen(true);

        let workspace = check_operations(vec![
            Operation::InsertElement {
                element: a.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: b.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: c.clone(),
                border_config: None,
            },
            Operation::FocusElement { element: b.clone() },
        ]);

        // Focusing should always removed fullscreen element
        assert_eq!(workspace.focused_tile_idx, 1);
        assert!(workspace.fullscreen.is_none());
        assert!(!c.fullscreen());
    }

    #[test]
    fn focus_next_element_removes_fullscreen() {
        let a = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let b = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let c = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        c.set_fullscreen(true);

        let workspace = check_operations(vec![
            Operation::InsertElement {
                element: a.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: b.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: c.clone(),
                border_config: None,
            },
            Operation::FocusNextElement,
        ]);

        // Focusing should always removed fullscreen element
        assert_eq!(workspace.focused_tile_idx, 2);
        assert!(workspace.fullscreen.is_none());
        assert!(!c.fullscreen());
    }

    #[test]
    fn focus_previous_element_removes_fullscreen() {
        let a = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let b = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let c = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        c.set_fullscreen(true);

        let workspace = check_operations(vec![
            Operation::InsertElement {
                element: a.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: b.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: c.clone(),
                border_config: None,
            },
            Operation::FocusPreviousElement,
        ]);

        // Focusing should always removed fullscreen element
        assert_eq!(workspace.focused_tile_idx, 0);
        assert!(workspace.fullscreen.is_none());
        assert!(!c.fullscreen());
    }

    #[test]
    fn swap_elements() {
        let a = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let b = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let c = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));

        let workspace = check_operations(vec![
            Operation::InsertElement {
                element: a.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: b.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: c.clone(),
                border_config: None,
            },
            Operation::SwapElements {
                a: a.clone(),
                b: b.clone(),
            },
            Operation::SwapElements {
                a: b.clone(),
                b: c.clone(),
            },
        ]);

        let a_idx = workspace.tiles.iter().position(|tile| *tile == a).unwrap();
        let b_idx = workspace.tiles.iter().position(|tile| *tile == b).unwrap();
        let c_idx = workspace.tiles.iter().position(|tile| *tile == c).unwrap();
        assert_eq!(a_idx, 1);
        assert_eq!(b_idx, 2);
        assert_eq!(c_idx, 0);
    }

    #[test]
    fn swap_with_next_element_removes_fullscreen() {
        let a = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let b = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let c = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        c.set_fullscreen(true);

        let workspace = check_operations(vec![
            Operation::InsertElement {
                element: a.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: b.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: c.clone(),
                border_config: None,
            },
            Operation::SwapWithNextElement, // swaps b and c
        ]);

        // Swapping should always removed fullscreen element
        assert!(workspace.fullscreen.is_none());
        assert!(!c.fullscreen());
        let a_idx = workspace.tiles.iter().position(|tile| *tile == a).unwrap();
        let b_idx = workspace.tiles.iter().position(|tile| *tile == b).unwrap();
        let c_idx = workspace.tiles.iter().position(|tile| *tile == c).unwrap();
        assert_eq!(a_idx, 0);
        assert_eq!(b_idx, 2);
        assert_eq!(c_idx, 1);
    }

    #[test]
    fn swap_with_previous_element_removes_fullscreen() {
        let a = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let b = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        let c = TestElement::new(Rectangle::from_loc_and_size((0, 0), (200, 200)));
        c.set_fullscreen(true);

        let workspace = check_operations(vec![
            Operation::InsertElement {
                element: a.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: b.clone(),
                border_config: None,
            },
            Operation::InsertElement {
                element: c.clone(),
                border_config: None,
            },
            Operation::SwapWithPreviousElement, // swaps c and b
        ]);

        // Swapping should always removed fullscreen element
        assert!(workspace.fullscreen.is_none());
        assert!(!c.fullscreen());
        let a_idx = workspace.tiles.iter().position(|tile| *tile == a).unwrap();
        let b_idx = workspace.tiles.iter().position(|tile| *tile == b).unwrap();
        let c_idx = workspace.tiles.iter().position(|tile| *tile == c).unwrap();
        assert_eq!(a_idx, 1);
        assert_eq!(b_idx, 0);
        assert_eq!(c_idx, 2);
    }
}
