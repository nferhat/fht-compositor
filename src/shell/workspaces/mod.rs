pub mod layout;
pub mod tile;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Physical, Point, Rectangle, Scale};

pub use self::layout::WorkspaceLayout;
use self::tile::{WorkspaceElement, WorkspaceTile, WorkspaceTileRenderElement};
use crate::config::{
    BorderConfig, InsertWindowStrategy, WorkspaceSwitchAnimationDirection, CONFIG,
};
use crate::fht_render_elements;
use crate::renderer::FhtRenderer;
use crate::utils::animation::Animation;
use crate::utils::geometry::{
    Global, Local, PointGlobalExt, PointLocalExt, RectExt, RectGlobalExt, RectLocalExt, SizeExt,
};
use crate::utils::output::OutputExt;

pub struct WorkspaceSet<E: WorkspaceElement> {
    /// The output of this set.
    pub(super) output: Output,

    /// All the workspaces of this set.
    pub workspaces: Vec<Workspace<E>>,

    /// The current switch animation, of any.
    pub switch_animation: Option<WorkspaceSwitchAnimation>,

    /// The active workspace index.
    pub(super) active_idx: AtomicUsize,
}

#[allow(dead_code)]
impl<E: WorkspaceElement> WorkspaceSet<E> {
    /// Create a new [`WorkspaceSet`] for this output.
    ///
    /// This function creates  9 workspaces, indexed from 0 to 8, each with independent layout
    /// window list. It's up to whatever manages this set to ensure focusing happens correctly, and
    /// that windows are getting mapped to the right set.
    pub fn new(output: Output) -> Self {
        let workspaces = (0..9).map(|_| Workspace::new(output.clone())).collect();
        Self {
            output: output.clone(),
            workspaces,
            switch_animation: None,
            active_idx: 0.into(),
        }
    }

    /// Refresh internal state of the [`WorkspaceSet`]
    ///
    /// Preferably call this before flushing clients.
    pub fn refresh(&mut self) {
        self.workspaces_mut().for_each(Workspace::refresh);
    }

    /// Reload the configuration of the [`WorkspaceSet`]
    pub fn reload_config(&mut self) {
        let layouts = CONFIG.general.layouts.clone();
        for workspace in &mut self.workspaces {
            workspace.layouts = layouts.clone();
            workspace.active_layout_idx = workspace
                .active_layout_idx
                .clamp(0, workspace.layouts.len() - 1);
        }
    }

    /// Set the active workspace index for this [`WorkspaceSet`], returning the possible focus
    /// candidate that the compositor should focus.
    ///
    /// Animations are opt-in, set `animate` to true if its needed.
    pub fn set_active_idx(&mut self, target_idx: usize, animate: bool) -> Option<E> {
        let target_idx = target_idx.clamp(0, 9);
        if !animate {
            self.active_idx.store(target_idx, Ordering::SeqCst);
            return self.workspaces[target_idx].focused().cloned();
        }

        let active_idx = self.active_idx.load(Ordering::SeqCst);
        if target_idx == active_idx || self.switch_animation.is_some() {
            return None;
        }

        self.switch_animation = Some(WorkspaceSwitchAnimation::new(target_idx));
        self.workspaces[target_idx].focused().cloned()
    }

    /// Get the active workspace index of this [`WorkspaceSet`]
    ///
    /// If there's a switch animation going on, use the target index and not the currently active
    /// one.
    pub fn get_active_idx(&self) -> usize {
        if let Some(WorkspaceSwitchAnimation { target_idx, .. }) = self.switch_animation.as_ref() {
            *target_idx
        } else {
            self.active_idx.load(Ordering::SeqCst)
        }
    }

    /// Get a reference to the active workspace.
    ///
    /// If there's a switch animation going on, use the target workspace and not the currently
    /// active one.
    pub fn active(&self) -> &Workspace<E> {
        if let Some(WorkspaceSwitchAnimation { target_idx, .. }) = self.switch_animation.as_ref() {
            &self.workspaces[*target_idx]
        } else {
            &self.workspaces[self.active_idx.load(Ordering::SeqCst)]
        }
    }

    /// Get a mutable reference to the active workspace.
    ///
    /// If there's a switch animation going on, use the target workspace and not the currently
    /// active one.
    pub fn active_mut(&mut self) -> &mut Workspace<E> {
        if let Some(WorkspaceSwitchAnimation { target_idx, .. }) = self.switch_animation.as_ref() {
            &mut self.workspaces[*target_idx]
        } else {
            &mut self.workspaces[self.active_idx.load(Ordering::SeqCst)]
        }
    }

    /// Get an iterator over all the [`Workspace`]s in this [`WorkspaceSet`]
    pub fn workspaces(&self) -> impl Iterator<Item = &Workspace<E>> {
        self.workspaces.iter()
    }

    /// Get a mutable iterator over all the [`Workspace`]s in this [`WorkspaceSet`]
    pub fn workspaces_mut(&mut self) -> impl Iterator<Item = &mut Workspace<E>> {
        self.workspaces.iter_mut()
    }

    /// Arrange the [`Workspace`]s and their windows.
    ///
    /// You need to call this when this [`WorkspaceSet`] output changes geometry to ensure that
    /// the tiled window geometries actually fill the output space.
    pub fn arrange(&mut self) {
        self.workspaces_mut().for_each(Workspace::arrange_tiles)
    }

    /// Find the window associated with this [`WlSurface`]
    pub fn find_element(&self, surface: &WlSurface) -> Option<&E> {
        self.workspaces().find_map(|ws| ws.find_element(surface))
    }

    /// Find the workspace containing the window associated with this [`WlSurface`].
    pub fn find_workspace(&self, surface: &WlSurface) -> Option<&Workspace<E>> {
        self.workspaces().find(|ws| ws.has_surface(surface))
    }

    /// Find the workspace containing the window associated with this [`WlSurface`].
    pub fn find_workspace_mut(&mut self, surface: &WlSurface) -> Option<&mut Workspace<E>> {
        self.workspaces_mut().find(|ws| ws.has_surface(surface))
    }

    /// Find the window associated with this [`WlSurface`] with the [`Workspace`] containing it.
    pub fn find_element_and_workspace(&self, surface: &WlSurface) -> Option<(&E, &Workspace<E>)> {
        self.workspaces()
            .find_map(|ws| ws.find_element(surface).map(|w| (w, ws)))
    }

    /// Find the window associated with this [`WlSurface`] with the [`Workspace`] containing it.
    pub fn find_element_and_workspace_mut(
        &mut self,
        surface: &WlSurface,
    ) -> Option<(E, &mut Workspace<E>)> {
        self.workspaces_mut()
            .find_map(|ws| ws.find_element(surface).cloned().map(|w| (w, ws)))
    }

    /// Get a reference to the [`Workspace`] holding this window, if any.
    pub fn ws_for(&self, element: &E) -> Option<&Workspace<E>> {
        self.workspaces().find(|ws| ws.has_element(element))
    }

    /// Get a mutable reference to the [`Workspace`] holding this window, if any.
    pub fn ws_mut_for(&mut self, element: &E) -> Option<&mut Workspace<E>> {
        self.workspaces_mut().find(|ws| ws.has_element(element))
    }

    /// Get the current fullscreend element and it's location in global coordinate space.
    ///
    /// This function also accounts for workspace switch animations.
    #[profiling::function]
    pub fn current_fullscreen(&self) -> Option<(&E, Point<i32, Global>)> {
        let active = self.active();
        let location = active.output.geometry().loc;
        active
            .fullscreen
            .as_ref()
            .map(|fs| (fs.inner.element(), location))
    }

    /// Get the element in under the cursor and it's location in global coordinate space.
    ///
    /// This function also accounts for workspace switch animations.
    #[profiling::function]
    pub fn element_under(&self, point: Point<f64, Global>) -> Option<(&E, Point<i32, Global>)> {
        if self.switch_animation.is_none() {
            // It's just the active one, so no need to do additional calculations.
            return self.active().element_under(point);
        }

        let animation = self.switch_animation.as_ref().unwrap();
        let output_geo = self.output.geometry();

        let (current_offset, target_offset) =
            if animation.target_idx > self.active_idx.load(Ordering::SeqCst) {
                // Focusing the next offset.
                // For the active, how much should we *remove* from the current position
                // For the target, how much should we add to the current position
                match CONFIG.animation.workspace_switch.direction {
                    WorkspaceSwitchAnimationDirection::Horizontal => {
                        let offset =
                            (animation.animation.value() * output_geo.size.w as f64).round() as i32;
                        (
                            Point::from(((-offset), 0)),
                            Point::from(((-offset + output_geo.size.w), 0)),
                        )
                    }
                    WorkspaceSwitchAnimationDirection::Vertical => {
                        let offset =
                            (animation.animation.value() * output_geo.size.h as f64).round() as i32;
                        (
                            Point::from((0, (-offset))),
                            Point::from((0, (-offset + output_geo.size.h))),
                        )
                    }
                }
            } else {
                // Focusing a previous workspace
                // For the active, how much should we add to tyhe current position
                // For the target, how much should we remove from the current position.
                match CONFIG.animation.workspace_switch.direction {
                    WorkspaceSwitchAnimationDirection::Horizontal => {
                        let offset =
                            (animation.animation.value() * output_geo.size.w as f64).round() as i32;
                        (
                            Point::from((offset, 0)),
                            Point::from((offset - output_geo.size.w, 0)),
                        )
                    }
                    WorkspaceSwitchAnimationDirection::Vertical => {
                        let offset =
                            (animation.animation.value() * output_geo.size.h as f64).round() as i32;
                        (
                            Point::from((0, (offset))),
                            Point::from((0, (offset - output_geo.size.h))),
                        )
                    }
                }
            };

        self.active()
            .element_under(point + current_offset.to_f64())
            .map(|(ft, loc)| (ft, loc + current_offset))
            .or_else(|| {
                self.workspaces[animation.target_idx]
                    .element_under(point + target_offset.to_f64())
                    .map(|(ft, loc)| (ft, loc + target_offset))
            })
    }

    /// Render all the elements in this workspace set, returning them and whether it currently
    /// holds a fullscreen element.
    #[profiling::function]
    pub fn render_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
    ) -> (bool, Vec<WorkspaceSetRenderElement<R>>) {
        let mut elements = vec![];
        let active = &self.workspaces[self.active_idx.load(Ordering::SeqCst)];
        let output_geo: Rectangle<i32, Physical> = self
            .output
            .geometry()
            .as_logical()
            .to_physical_precise_round(scale);

        // No switch, just give what's active.
        let active_elements = active.render_elements(renderer, scale);
        let Some(animation) = self.switch_animation.as_ref() else {
            elements.extend(
                active_elements
                    .into_iter()
                    .map(WorkspaceSetRenderElement::Normal),
            );

            return (false, elements);
        };

        // Switching
        let target = &self.workspaces[animation.target_idx];
        let target_elements = target.render_elements(renderer, scale);

        // Switch finished, avoid blank frame and return target elements immediatly
        if animation.animation.is_finished() {
            self.active_idx
                .store(animation.target_idx, Ordering::SeqCst);
            elements.extend(
                target_elements
                    .into_iter()
                    .map(WorkspaceSetRenderElement::Normal),
            );
            return (false, elements);
        }

        // Otherwise to computations
        let (current_offset, target_offset) =
            if animation.target_idx > self.active_idx.load(Ordering::SeqCst) {
                // Focusing the next offset.
                // For the active, how much should we *remove* from the current position
                // For the target, how much should we add to the current position
                match CONFIG.animation.workspace_switch.direction {
                    WorkspaceSwitchAnimationDirection::Horizontal => {
                        let offset =
                            (animation.animation.value() * output_geo.size.w as f64).round() as i32;
                        (
                            Point::from(((-offset), 0)),
                            Point::from(((-offset + output_geo.size.w), 0)),
                        )
                    }
                    WorkspaceSwitchAnimationDirection::Vertical => {
                        let offset =
                            (animation.animation.value() * output_geo.size.h as f64).round() as i32;
                        (
                            Point::from((0, (-offset))),
                            Point::from((0, (-offset + output_geo.size.h))),
                        )
                    }
                }
            } else {
                // Focusing a previous workspace
                // For the active, how much should we add to tyhe current position
                // For the target, how much should we remove from the current position.
                match CONFIG.animation.workspace_switch.direction {
                    WorkspaceSwitchAnimationDirection::Horizontal => {
                        let offset =
                            (animation.animation.value() * output_geo.size.w as f64).round() as i32;
                        (
                            Point::from((offset, 0)),
                            Point::from((offset - output_geo.size.w, 0)),
                        )
                    }
                    WorkspaceSwitchAnimationDirection::Vertical => {
                        let offset =
                            (animation.animation.value() * output_geo.size.h as f64).round() as i32;
                        (
                            Point::from((0, (offset))),
                            Point::from((0, (offset - output_geo.size.h))),
                        )
                    }
                }
            };

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
            false, // active.fullscreen.is_some() || target.fullscreen.is_some(),
            elements,
        )
    }
}

/// An active workspace switching animation
pub struct WorkspaceSwitchAnimation {
    /// The underlying animation tweener to generate values
    pub animation: Animation,
    /// Which workspace are we going to focus.
    pub target_idx: usize,
}

impl WorkspaceSwitchAnimation {
    /// Create a new [`WorkspaceSwitchAnimation`]
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
}

fht_render_elements! {
    WorkspaceSetRenderElement<R> => {
        Normal = WorkspaceTileRenderElement<R>,
        Switching = RelocateRenderElement<WorkspaceTileRenderElement<R>>,
    }
}

/// A single workspace.
///
/// This workspace should not stand on it's own, and it's preferred you use it with a
/// [`WorkspaceSet`], but nothing stops you from doing whatever you want with it like assigning it
/// to a single output.
#[derive(Debug)]
pub struct Workspace<E: WorkspaceElement> {
    /// The output for this workspace
    pub output: Output,

    /// The tiles this workspace contains.
    ///
    /// These must all have valid [`WlSurface`]s (aka: being mapped), otherwise the workspace inner
    /// logic will PANIC.
    pub tiles: Vec<WorkspaceTile<E>>,

    /// The focused window index.
    focused_tile_idx: usize,

    /// The layouts list for this workspace.
    pub layouts: Vec<WorkspaceLayout>,

    /// The currently fullscreened tile.
    pub fullscreen: Option<FullscreenTile<E>>,

    /// The active layout index.
    pub active_layout_idx: usize,
}

impl<E: WorkspaceElement> Workspace<E> {
    /// Create a new [`Workspace`] for this output.
    pub fn new(output: Output) -> Self {
        Self {
            output,
            tiles: vec![],
            focused_tile_idx: 0,
            layouts: CONFIG.general.layouts.clone(),
            active_layout_idx: 0,
            fullscreen: None,
        }
    }

    /// Get an iterator over this workspace's tiles.
    ///
    /// This does NOT include the fullscreened tile!
    pub fn tiles(&self) -> impl Iterator<Item = &WorkspaceTile<E>> {
        self.tiles.iter()
    }

    /// Refresh internal state of the [`Workspace`]
    ///
    /// Preferably call this before flushing clients.
    #[profiling::function]
    pub fn refresh(&mut self) {
        let output_geometry = self.output.geometry();

        let mut should_refresh_geometries = self
            .fullscreen
            .take_if(|fs| !fs.inner.element.alive())
            .is_some();

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

            let mut bbox = fullscreen.inner.element.bbox().as_global();
            bbox.loc = fullscreen.inner.location.to_global(&self.output);
            if let Some(mut overlap) = output_geometry.intersection(bbox) {
                // output_enter excepts the overlap to be relative to the element, weird choice
                // bu I comply.
                overlap.loc -= bbox.loc;
                fullscreen
                    .inner
                    .element
                    .output_enter(&self.output, overlap.as_logical());
            }

            fullscreen.inner.send_pending_configure();
            fullscreen.inner.element.refresh();
        }

        // Clean dead/zombie tiles
        // Also ensure that we dont try to access out of bounds indexes, and sync up the IPC.
        let mut removed_ids = vec![];
        self.tiles.retain(|tile| {
            if !tile.element.alive() {
                removed_ids.push(tile.element.uid());
                false
            } else {
                true
            }
        });
        let new_len = self.tiles.len();
        if !removed_ids.is_empty() {
            should_refresh_geometries = true;
        }

        if should_refresh_geometries {
            self.focused_tile_idx = self.focused_tile_idx.clamp(0, new_len.saturating_sub(1));
            self.arrange_tiles();
        }

        // Refresh internal state of windows
        for (idx, tile) in self.tiles.iter_mut().enumerate() {
            // This is now managed globally with focus targets
            tile.element.set_activated(idx == self.focused_tile_idx);

            let mut bbox = tile.element.bbox().as_global();
            bbox.loc = tile.location.to_global(&self.output);
            if let Some(mut overlap) = output_geometry.intersection(bbox) {
                // output_enter excepts the overlap to be relative to the element, weird choice
                // bu I comply.
                overlap.loc -= bbox.loc;
                tile.element
                    .output_enter(&self.output, overlap.as_logical());
            }

            tile.send_pending_configure();
            tile.element.refresh();
        }
    }

    /// Find the element with this [`WlSurface`]
    pub fn find_element(&self, surface: &WlSurface) -> Option<&E> {
        self.fullscreen
            .as_ref()
            .and_then(|fs| {
                fs.inner
                    .has_surface(surface, WindowSurfaceType::ALL)
                    .then_some(&fs.inner.element)
            })
            .or_else(|| {
                self.tiles.iter().find_map(|tile| {
                    tile.has_surface(surface, WindowSurfaceType::ALL)
                        .then_some(&tile.element)
                })
            })
    }

    /// Find the tile with this [`WlSurface`]
    pub fn find_tile(&mut self, surface: &WlSurface) -> Option<&mut WorkspaceTile<E>> {
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

    /// Find the tile with this [`WlSurface`]
    pub fn tile_mut_for(&mut self, element: &E) -> Option<&mut WorkspaceTile<E>> {
        self.fullscreen
            .as_mut()
            .filter(|fs| fs.inner == *element)
            .map(|fs| &mut fs.inner)
            .or_else(|| self.tiles.iter_mut().find(|tile| *tile == element))
    }

    /// Return whether this workspace contains this element.
    pub fn has_element(&self, element: &E) -> bool {
        let mut ret = false;
        ret |= self
            .fullscreen
            .as_ref()
            .is_some_and(|fs| fs.inner == *element);
        ret |= self.tiles.iter().any(|tile| tile == element);
        ret
    }

    /// Return whether this workspace has an element  with this [`WlSurface`].
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

    /// Return the focused element, giving priority to the fullscreen element first, then the
    /// possible active non-fullscreen element.
    pub fn focused(&self) -> Option<&E> {
        if let Some(fullscreen) = self.fullscreen.as_ref() {
            return Some(&fullscreen.inner.element);
        }

        self.tiles
            .get(self.focused_tile_idx)
            .map(WorkspaceTile::element)
    }

    /// Return the focused tile, giving priority to the fullscreen elementj first, then the
    /// possible active non-fullscreen element.
    pub fn focused_tile(&self) -> Option<&WorkspaceTile<E>> {
        if let Some(fullscreen) = self.fullscreen.as_ref() {
            return Some(&fullscreen.inner);
        }
        self.tiles.get(self.focused_tile_idx)
    }

    /// Return the focused tile, giving priority to the fullscreen elementj first, then the
    /// possible active non-fullscreen element.
    pub fn focused_tile_mut(&mut self) -> Option<&mut WorkspaceTile<E>> {
        if let Some(fullscreen) = self.fullscreen.as_mut() {
            return Some(&mut fullscreen.inner);
        }
        self.tiles.get_mut(self.focused_tile_idx)
    }

    /// Get the global geometry of a given element.
    pub fn element_geometry(&self, element: &E) -> Option<Rectangle<i32, Global>> {
        if self
            .fullscreen
            .as_ref()
            .is_some_and(|fs| fs.inner == *element)
        {
            return Some(self.output.geometry());
        }
        self.tiles
            .iter()
            .find(|tile| *tile == element)
            .map(|tile| tile.geometry().to_global(&self.output))
    }

    /// Get the visual global geometry of a given element.
    ///
    /// See [`WorkspaceTile::visual_geometry`]
    pub fn element_visual_geometry(&self, element: &E) -> Option<Rectangle<i32, Global>> {
        if self
            .fullscreen
            .as_ref()
            .is_some_and(|fs| fs.inner == *element)
        {
            return Some(self.output.geometry());
        }
        self.tiles
            .iter()
            .find(|tile| *tile == element)
            .map(|tile| tile.visual_geometry().to_global(&self.output))
    }

    /// Insert a tile in this [`Workspace`]
    ///
    /// See [`Workspace::insert_element`]
    pub fn insert_tile(&mut self, tile: WorkspaceTile<E>) {
        let WorkspaceTile {
            element,
            border_config,
            ..
        } = tile;
        self.insert_element(element, border_config);
    }

    /// Insert an element in this [`Workspace`]
    ///
    /// This function does additional configuration of the element before creating a tile for it,
    /// mainly setting the bounds of the window, and notifying it of entering this
    /// [`Workspace`] output.
    ///
    /// This doesn't reinsert the element if it's already inserted.
    pub fn insert_element(&mut self, element: E, border_config: Option<BorderConfig>) {
        if self.has_element(&element) {
            return;
        }

        // Output overlap + wl_surface scale and transform will be set when using self.refresh
        element.set_bounds(Some(self.output.geometry().size.as_local()));
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

        self.arrange_tiles();
    }

    /// Removes a tile from this [`Workspace`], returning it if it was found.
    ///
    /// This function also undones the configuration that was done in [`Self::insert_window`]
    pub fn remove_tile(&mut self, element: &E) -> Option<WorkspaceTile<E>> {
        if self
            .fullscreen
            .as_ref()
            .is_some_and(|fs| fs.inner == *element)
        {
            let FullscreenTile { inner, .. } = self.take_fullscreen().unwrap();
            self.arrange_tiles();

            return Some(inner);
        }

        let Some(idx) = self.tiles.iter().position(|t| t.element == *element) else {
            return None;
        };

        let tile = self.tiles.remove(idx);
        // "Un"-configure the window (for potentially inserting it on another workspace who knows)
        tile.element.output_leave(&self.output);
        tile.element.set_bounds(None);
        self.focused_tile_idx = self
            .focused_tile_idx
            .clamp(0, self.tiles.len().saturating_sub(1));

        self.arrange_tiles();
        Some(tile)
    }

    /// Focus a given element, if this [`Workspace`] contains it.
    pub fn focus_element(&mut self, window: &E) {
        if let Some(idx) = self.tiles.iter().position(|w| w == window) {
            if let Some(FullscreenTile {
                inner,
                last_known_idx,
            }) = self.take_fullscreen()
            {
                self.tiles.insert(last_known_idx, inner);
                self.arrange_tiles();
            }

            self.focused_tile_idx = idx;

            self.refresh();
        }
    }

    /// Focus the next available element, cycling back to the first one if needed.
    pub fn focus_next_element(&mut self) -> Option<&E> {
        if self.tiles.is_empty() {
            return None;
        }

        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
            self.arrange_tiles();
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

    /// Focus the previous available element, cyclying all the way to the last element if needed.
    pub fn focus_previous_element(&mut self) -> Option<&E> {
        if self.tiles.is_empty() {
            return None;
        }

        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
            self.arrange_tiles();
        }

        let windows_len = self.tiles.len();
        self.focused_tile_idx = match self.focused_tile_idx.checked_sub(1) {
            Some(idx) => idx,
            None => windows_len - 1,
        };

        let tile = &self.tiles[self.focused_tile_idx];
        Some(tile.element())
    }

    /// Swap the two given elements.
    ///
    /// This will give the focus to b
    pub fn swap_elements(&mut self, a: &E, b: &E) {
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
        self.arrange_tiles();
    }

    /// Swap the current element with the next element.
    pub fn swap_with_next_element(&mut self) {
        if self.tiles.len() < 2 {
            return;
        }

        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
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
        self.arrange_tiles();
    }

    /// Swap the current element with the previous element.
    pub fn swap_with_previous_element(&mut self) {
        if self.tiles.len() < 2 {
            return;
        }

        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
        }

        let tiles_len = self.tiles.len();
        let last_focused_idx = self.focused_tile_idx;

        let new_focused_idx = match self.focused_tile_idx.checked_sub(1) {
            Some(idx) => idx,
            None => tiles_len - 1,
        };

        self.focused_tile_idx = new_focused_idx;
        self.tiles.swap(last_focused_idx, new_focused_idx);
        self.arrange_tiles();
    }

    /// Get the area used to tile the elements.
    ///
    /// This is the area that is used with [`Self::arrange_tiles`]
    pub fn tile_area(&self) -> Rectangle<i32, Local> {
        let mut area = layer_map_for_output(&self.output)
            .non_exclusive_zone()
            .as_local();
        let outer_gaps = CONFIG.general.outer_gaps;
        area.size -= (2 * outer_gaps, 2 * outer_gaps).into();
        area.loc += (outer_gaps, outer_gaps).into();
        area
    }

    /// Refresh the geometries of the tiles contained in this [`Workspace`].
    ///
    /// This ensures geometry for maximized and tiled elements.
    #[profiling::function]
    pub fn arrange_tiles(&mut self) {
        if let Some(FullscreenTile { inner, .. }) = self.fullscreen.as_mut() {
            // NOTE: Output top left is always (0,0) locally
            let mut output_geo = self.output.geometry().as_logical().as_local();
            output_geo.loc = (0, 0).into();
            inner.set_geometry(output_geo);
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
            .partition::<Vec<_>, _>(|tile| tile.element.maximized());

        for tile in maximized {
            tile.set_geometry(tile_area)
        }

        if tiled.is_empty() {
            return;
        }

        let tiled_len = tiled.len();
        layout.arrange_tiles(tiled.into_iter(), tiled_len, tile_area, inner_gaps);
    }

    /// Get the active layout that arranges the tiles
    pub fn get_active_layout(&self) -> WorkspaceLayout {
        self.layouts[self.active_layout_idx]
    }

    /// Select the next available layout in this [`Workspace`], cycling back to the first one if
    /// needed.
    pub fn select_next_layout(&mut self) {
        let layouts_len = self.layouts.len();
        let new_active_idx = self.active_layout_idx + 1;
        let new_active_idx = if new_active_idx == layouts_len {
            0
        } else {
            new_active_idx
        };

        self.active_layout_idx = new_active_idx;
        self.arrange_tiles();
    }

    /// Select the previous available layout in this [`Workspace`], cycling all the way back to the
    /// last layout if needed.
    pub fn select_previous_layout(&mut self) {
        let layouts_len = self.layouts.len();
        let new_active_idx = match self.active_layout_idx.checked_sub(1) {
            Some(idx) => idx,
            None => layouts_len - 1,
        };

        self.active_layout_idx = new_active_idx;
        self.arrange_tiles();
    }

    /// Change the master_width_factor of the active [`WorkspaceLayout`]
    ///
    /// This clamps the value between (0.05..=0.95).
    pub fn change_mwfact(&mut self, delta: f32) {
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
        self.arrange_tiles();
    }

    /// Change the nmaster of the active [`WorkspaceLayout`]
    ///
    /// This clamps the value between (1.0, +inf).
    pub fn change_nmaster(&mut self, delta: i32) {
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
        self.arrange_tiles();
    }

    /// Get the element under the pointer in this workspace.
    #[profiling::function]
    pub fn element_under(&self, point: Point<f64, Global>) -> Option<(&E, Point<i32, Global>)> {
        let point = point.to_local(&self.output);

        if let Some(FullscreenTile { inner: tile, .. }) = self.fullscreen.as_ref() {
            let render_location = tile.render_location();
            if tile.bbox().to_f64().contains(point)
                && tile
                    .element
                    .is_in_input_region(&(point - render_location.to_f64()).as_logical())
            {
                return Some((tile.element(), render_location.to_global(&self.output)));
            }
        }

        if let Some(tile) = self.focused_tile() {
            let render_location = tile.render_location();
            if tile.bbox().to_f64().contains(point)
                && tile
                    .element
                    .is_in_input_region(&(point - render_location.to_f64()).as_logical())
            {
                return Some((tile.element(), render_location.to_global(&self.output)));
            }
        }

        self.tiles
            .iter()
            .filter(|tile| tile.bbox().to_f64().contains(point))
            .find_map(|tile| {
                let render_location = tile.render_location();
                if tile
                    .element
                    .is_in_input_region(&(point - render_location.to_f64()).as_logical())
                {
                    Some((tile.element(), render_location.to_global(&self.output)))
                } else {
                    None
                }
            })
    }

    /// Get the elements under the pointer in this workspace.
    #[profiling::function]
    pub fn tiles_under(
        &self,
        point: Point<f64, Global>,
    ) -> impl Iterator<Item = &WorkspaceTile<E>> {
        let point = point.to_local(&self.output);

        None.into_iter()
            .chain(self.fullscreen.as_ref().map(|fs| &fs.inner))
            .chain(self.tiles.iter().filter(move |tile| {
                if !tile.bbox().to_f64().contains(point) {
                    return false;
                }

                let render_location = tile.render_location();
                tile.element
                    .is_in_input_region(&(point - render_location.to_f64()).as_logical())
            }))
    }

    /// Render all elements in this [`Workspace`], respecting the window's Z-index.
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
                    &self.output,
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
                &self.output,
                scale,
                CONFIG.decoration.focused_window_opacity,
                true,
            ));
        }

        for (idx, tile) in self.tiles().enumerate() {
            if idx == self.focused_tile_idx {
                continue;
            }

            render_elements.extend(tile.render_elements(
                renderer,
                &self.output,
                scale,
                CONFIG.decoration.normal_window_opacity,
                false,
            ));
        }

        render_elements
    }

    /// Remove the currently fullscreened tile.
    ///
    /// This also configures the element state.
    pub fn take_fullscreen(&mut self) -> Option<FullscreenTile<E>> {
        self.fullscreen.take().map(|mut fs| {
            fs.inner.element.output_leave(&self.output);
            fs.inner.element.set_bounds(None);
            fs.inner.element.set_fullscreen(false);
            fs.inner.element.set_fullscreen_output(None);
            fs.inner.send_pending_configure();

            fs
        })
    }

    /// Fullscreen an element.
    pub fn fullscreen_element(&mut self, element: &E) {
        if let Some(FullscreenTile {
            inner,
            last_known_idx,
        }) = self.take_fullscreen()
        {
            self.tiles.insert(last_known_idx, inner);
        }
        dbg!(element);

        let Some(idx) = self.tiles.iter().position(|t| t == element) else {
            return;
        };
        let tile = self.remove_tile(element).unwrap();
        tile.element.set_fullscreen(true);
        self.fullscreen = Some(FullscreenTile {
            inner: tile,
            last_known_idx: idx,
        });
        self.arrange_tiles();
    }
}

#[derive(Debug)]
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
