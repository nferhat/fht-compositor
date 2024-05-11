pub mod layout;
pub mod tile;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_std::task::spawn;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::desktop::layer_map_for_output;
use smithay::output::Output;
use smithay::reexports::calloop::{self, LoopHandle, RegistrationToken};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Physical, Point, Rectangle, Scale};

pub use self::layout::WorkspaceLayout;
use self::tile::{WorkspaceElement, WorkspaceTile, WorkspaceTileRenderElement};
use crate::config::{BorderConfig, WorkspaceSwitchAnimationDirection, CONFIG};
use crate::fht_render_elements;
use crate::ipc::{IpcOutput, IpcWorkspace, IpcWorkspaceRequest};
use crate::renderer::FhtRenderer;
use crate::state::State;
use crate::utils::animation::Animation;
use crate::utils::dbus::DBUS_CONNECTION;
use crate::utils::geometry::{
    Global, PointGlobalExt, PointLocalExt, RectExt, RectGlobalExt, RectLocalExt, SizeExt,
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
    pub fn new(output: Output, loop_handle: LoopHandle<'static, State>) -> Self {
        let mut workspaces = vec![];
        let name = output.name().replace("-", "_");
        let path_base = format!("/fht/desktop/Compositor/Output/{name}");

        for index in 0..9 {
            let output = output.clone();
            let loop_handle = loop_handle.clone();
            let ipc_path = format!("{path_base}/Workspaces/{index}");
            workspaces.push(Workspace::new(output, loop_handle, index == 0, ipc_path));
        }

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

        {
            let name = self.output.name().replace("-", "_");
            let path = format!("/fht/desktop/Compositor/Output/{name}");
            let target_idx = target_idx as u8;
            spawn(async move {
                let iface_ref = DBUS_CONNECTION
                    .object_server()
                    .inner()
                    .interface::<_, IpcOutput>(path)
                    .await
                    .unwrap();
                let mut iface = iface_ref.get_mut().await;
                iface.active_workspace_index = target_idx;
                iface
                    .active_workspace_index_changed(iface_ref.signal_context())
                    .await
                    .unwrap();
            });
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
        // TODO: Reimplement fullscreen
        None
    }

    /// Get the element in under the cursor and it's location in global coordinate space.
    ///
    /// This function also accounts for workspace switch animations.
    #[profiling::function]
    pub fn window_under(&self, point: Point<f64, Global>) -> Option<(&E, Point<i32, Global>)> {
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
        alpha: f32,
    ) -> (bool, Vec<WorkspaceSetRenderElement<R>>) {
        let mut elements = vec![];
        let active = &self.workspaces[self.active_idx.load(Ordering::SeqCst)];
        let output_geo: Rectangle<i32, Physical> = self
            .output
            .geometry()
            .as_logical()
            .to_physical_precise_round(scale);

        // No switch, just give what's active.
        let active_elements = active.render_elements(renderer, scale, alpha);
        if self.switch_animation.is_none() {
            elements.extend(
                active_elements
                    .into_iter()
                    .map(WorkspaceSetRenderElement::Normal),
            );

            return (false, elements);
        }

        // Switching
        let animation = self.switch_animation.as_ref().unwrap();
        let target = &self.workspaces[animation.target_idx];
        let target_elements = target.render_elements(renderer, scale, alpha);

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
        );

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
    ///
    /// WARNING: We shouldn't expose this to keep the dbus interface in sync, but here its symbol
    /// to drain the windows when deleting an output, soo it should be fine
    pub tiles: Vec<WorkspaceTile<E>>,

    /// The focused window index.
    focused_tile_idx: usize,

    // TODO: Reimplement fullscreening
    /// The layouts list for this workspace.
    pub layouts: Vec<WorkspaceLayout>,

    /// The active layout index.
    active_layout_idx: usize,

    // Using an Arc is fine since workspaces are static to each output, so the ipc_path should
    // never be able to change.
    //
    // Thank you logan smith for this simple tip.
    pub ipc_path: Arc<str>,
    ipc_token: RegistrationToken,
    loop_handle: LoopHandle<'static, State>,
}

impl<E: WorkspaceElement> Drop for Workspace<E> {
    fn drop(&mut self) {
        // When dropping thw workspace, we also want to close the MPSC channel opened with it to
        // communicate with the async dbus api.
        //
        // Dropping the dbus object path should drop the `IpcWorkspace` struct that holds the
        // sender, removing the ipc token from the event loop removes the callback and with it the
        // receiver, and thus dropping our channel
        self.loop_handle.remove(self.ipc_token);

        let ipc_path = self.ipc_path.clone();
        async_std::task::spawn(async move {
            match DBUS_CONNECTION
                .object_server()
                .inner()
                .remove::<IpcWorkspace, _>(ipc_path.as_ref())
                .await
            {
                Err(err) => warn!(?err, "Failed to unadvertise workspace from IPC!"),
                Ok(destroyed) => assert!(destroyed),
            }
        });
    }
}

impl<E: WorkspaceElement> Workspace<E> {
    /// Create a new [`Workspace`] for this output.
    pub fn new(
        output: Output,
        loop_handle: LoopHandle<'static, State>,
        active: bool,
        ipc_path: String,
    ) -> Self {
        // IPC stuff.
        let (ipc_workspace, channel) = IpcWorkspace::new(active, "bstack".into());
        assert!(DBUS_CONNECTION
            .object_server()
            .at(ipc_path.as_str(), ipc_workspace)
            .unwrap());

        let ipc_path_2 = ipc_path.clone();
        let ipc_token = loop_handle
            .insert_source(channel, move |event, (), state| {
                let calloop::channel::Event::Msg(req) = event else {
                    return;
                };
                state.handle_workspace_ipc_request(&ipc_path_2, req);
            })
            .expect("Failed to insert workspace IPC source!");

        Self {
            output,

            tiles: vec![],
            // fullscreen: None,
            focused_tile_idx: 0,

            layouts: CONFIG.general.layouts.clone(),
            active_layout_idx: 0,

            ipc_path: ipc_path.as_str().into(),
            ipc_token,
            loop_handle,
        }
    }

    /// Refresh internal state of the [`Workspace`]
    ///
    /// Preferably call this before flushing clients.
    #[profiling::function]
    pub fn refresh(&mut self) {
        let mut should_refresh_geometries = false;
        // Invalidate current fullscreen if its dead

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

            {
                let ipc_path = self.ipc_path.clone();
                spawn(async move {
                    let iface_ref = DBUS_CONNECTION
                        .object_server()
                        .inner()
                        .interface::<_, IpcWorkspace>(ipc_path.as_ref())
                        .await
                        .unwrap();
                    let mut iface = iface_ref.get_mut().await;
                    iface.windows.retain(|uid| !removed_ids.contains(uid));
                    iface
                        .windows_changed(iface_ref.signal_context())
                        .await
                        .unwrap();
                });
            }
        }

        if should_refresh_geometries {
            self.focused_tile_idx = self.focused_tile_idx.clamp(0, new_len.saturating_sub(1));
            self.arrange_tiles();
        }

        // Refresh internal state of windows
        let output_geometry = self.output.geometry();
        for (idx, tile) in self.tiles.iter().enumerate() {
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

            tile.element.send_pending_configure();
            tile.element.refresh();
        }
    }

    /// Find the element with this [`WlSurface`]
    pub fn find_element(&self, surface: &WlSurface) -> Option<&E> {
        self.tiles.iter().find_map(|tile| {
            (tile.element.wl_surface().as_ref() == Some(surface)).then_some(&tile.element)
        })
    }

    /// Find the tile with this [`WlSurface`]
    pub fn tile_for(&self, element: &E) -> Option<&WorkspaceTile<E>> {
        self.tiles.iter().find(|tile| *tile == element)
    }

    /// Find the tile with this [`WlSurface`]
    pub fn tile_mut_for(&mut self, element: &E) -> Option<&mut WorkspaceTile<E>> {
        self.tiles.iter_mut().find(|tile| *tile == element)
    }

    /// Return whether this workspace contains this element.
    pub fn has_element(&self, window: &E) -> bool {
        self.tiles.iter().any(|tile| tile.element == *window)
    }

    /// Return whether this workspace has an element  with this [`WlSurface`].
    pub fn has_surface(&self, surface: &WlSurface) -> bool {
        self.tiles
            .iter()
            .any(|tile| tile.element.wl_surface().as_ref() == Some(surface))
    }

    /// Return the focused element, giving priority to the fullscreen element first, then the
    /// possible active non-fullscreen element.
    pub fn focused(&self) -> Option<&E> {
        self.tiles
            .get(self.focused_tile_idx)
            .map(WorkspaceTile::element)
    }

    /// Return the focused tile, giving priority to the fullscreen elementj first, then the
    /// possible active non-fullscreen element.
    pub fn focused_tile_mut(&mut self) -> Option<&mut WorkspaceTile<E>> {
        self.tiles.get_mut(self.focused_tile_idx)
    }

    /// Get the global location of a given element.
    pub fn element_location(&self, element: &E) -> Option<Point<i32, Global>> {
        self.tiles
            .iter()
            .find(|tile| tile.element == *element)
            .map(|tile| tile.location.to_global(&self.output))
    }

    /// Get the global geometry of a given element.
    pub fn element_geometry(&self, element: &E) -> Option<Rectangle<i32, Global>> {
        self.tiles
            .iter()
            .find(|tile| *tile == element)
            .map(|tile| tile.geometry().to_global(&self.output))
    }

    /// Insert a tile in this [`Workspace`]
    ///
    /// See [`Workspace::insert_element`]
    pub fn insert_tile(&mut self, tile: WorkspaceTile<E>) {
        let WorkspaceTile { element, border_config, .. } = tile;
        self.insert_element(element, border_config);
    }

    /// Insert an element in this [`Workspace`]
    ///
    /// This function does additional configuration of the element before creating a tile for it,
    /// mainly setting the bounds of the window, and notifying it of entering this
    /// [`Workspace`] output.
    ///
    /// This doesn't reinsert the element if it's already inserted.
    pub fn insert_element(
        &mut self,
        window: E,
        border_config: Option<BorderConfig>, // additional config for the tile.
    ) {
        if self.tiles.iter().any(|t| *t == window) {
            return;
        }

        // Output overlap + wl_surface scale and transform will be set when using self.refresh
        window.set_bounds(Some(self.output.geometry().size.as_local()));

        {
            let ipc_path = self.ipc_path.clone();
            let uid = window.uid();
            spawn(async move {
                let iface_ref = DBUS_CONNECTION
                    .object_server()
                    .inner()
                    .interface::<_, IpcWorkspace>(ipc_path.as_ref())
                    .await
                    .unwrap();
                let mut iface = iface_ref.get_mut().await;
                iface.windows.push(uid);
                iface
                    .windows_changed(iface_ref.signal_context())
                    .await
                    .unwrap();
            });
        }

        let tile = WorkspaceTile::new(window, border_config);
        self.tiles.push(tile);
        if CONFIG.general.focus_new_windows {
            self.focused_tile_idx = self.tiles.len() - 1;
        }
        self.arrange_tiles();
    }

    /// Removes a tile from this [`Workspace`], returning it if it was found.
    ///
    /// This function also undones the configuration that was done in [`Self::insert_window`]
    pub fn remove_tile(&mut self, element: &E) -> Option<WorkspaceTile<E>> {
        let Some(idx) = self.tiles.iter().position(|t| t.element == *element) else {
            return None;
        };

        let tile = self.tiles.remove(idx);
        // "Un"-configure the window (for potentially inserting it on another workspace who knows)
        tile.element.output_leave(&self.output);
        tile.element.set_bounds(None);
        self.focused_tile_idx = self.focused_tile_idx.clamp(0, self.tiles.len() - 1);

        {
            let ipc_path = self.ipc_path.clone();
            let window_id = tile.element.uid();
            spawn(async move {
                let iface_ref = DBUS_CONNECTION
                    .object_server()
                    .inner()
                    .interface::<_, IpcWorkspace>(ipc_path.as_ref())
                    .await
                    .unwrap();
                let mut iface = iface_ref.get_mut().await;
                iface.windows.retain(|uid| *uid != window_id);
                iface
                    .windows_changed(iface_ref.signal_context())
                    .await
                    .unwrap();
            });
        }

        self.arrange_tiles();
        Some(tile)
    }

    /// Removes an element from this [`Workspace`], returning it if it was found.
    ///
    /// This function also undones the configuration that was done in [`Self::insert_window`]
    pub fn remove_element(&mut self, element: &E) -> Option<E> {
        self.remove_tile(element).map(|t| t.element)
    }

    /// Focus a given element, if this [`Workspace`] contains it.
    pub fn focus_element(&mut self, window: &E) {
        if let Some(idx) = self.tiles.iter().position(|w| w == window) {
            self.focused_tile_idx = idx;

            {
                let ipc_path = self.ipc_path.clone();
                spawn(async move {
                    let iface_ref = DBUS_CONNECTION
                        .object_server()
                        .inner()
                        .interface::<_, IpcWorkspace>(ipc_path.as_ref())
                        .await
                        .unwrap();
                    let mut iface = iface_ref.get_mut().await;
                    iface.focused_window_index = idx as u8;
                    iface
                        .focused_window_changed(iface_ref.signal_context())
                        .await
                        .unwrap();
                });
            }

            self.refresh();
        }
    }

    /// Focus the next available element, cycling back to the first one if needed.
    pub fn focus_next_element(&mut self) -> Option<&E> {
        if self.tiles.is_empty() {
            return None;
        }

        let tiles_len = self.tiles.len();
        let new_focused_idx = self.focused_tile_idx + 1;
        self.focused_tile_idx = if new_focused_idx == tiles_len {
            0
        } else {
            new_focused_idx
        };

        {
            let ipc_path = self.ipc_path.clone();
            let focused_tile_idx = self.focused_tile_idx as u8;
            spawn(async move {
                let iface_ref = DBUS_CONNECTION
                    .object_server()
                    .inner()
                    .interface::<_, IpcWorkspace>(ipc_path.as_ref())
                    .await
                    .unwrap();
                let mut iface = iface_ref.get_mut().await;
                iface.focused_window_index = focused_tile_idx;
                iface
                    .focused_window_changed(iface_ref.signal_context())
                    .await
                    .unwrap();
            });
        }

        let tile = &self.tiles[self.focused_tile_idx];
        Some(tile.element())
    }

    /// Focus the previous available element, cyclying all the way to the last element if needed.
    pub fn focus_previous_element(&mut self) -> Option<&E> {
        if self.tiles.is_empty() {
            return None;
        }

        let windows_len = self.tiles.len();
        self.focused_tile_idx = match self.focused_tile_idx.checked_sub(1) {
            Some(idx) => idx,
            None => windows_len - 1,
        };

        {
            let ipc_path = self.ipc_path.clone();
            let focused_tile_idx = self.focused_tile_idx as u8;
            spawn(async move {
                let iface_ref = DBUS_CONNECTION
                    .object_server()
                    .inner()
                    .interface::<_, IpcWorkspace>(ipc_path.as_ref())
                    .await
                    .unwrap();
                let mut iface = iface_ref.get_mut().await;
                iface.focused_window_index = focused_tile_idx;
                iface
                    .focused_window_changed(iface_ref.signal_context())
                    .await
                    .unwrap();
            });
        }

        let tile = &self.tiles[self.focused_tile_idx];
        Some(tile.element())
    }

    /// Swap the two given elements.
    ///
    /// This will give the focus to b
    pub fn swap_elements(&mut self, a: &E, b: &E) {
        let Some(a_idx) = self.tiles.iter().position(|tile| tile.element == *a) else { return };
        let Some(b_idx) = self.tiles.iter().position(|tile| tile.element == *b) else { return };
        self.focused_tile_idx = b_idx;
        self.tiles.swap(a_idx, b_idx);
        self.arrange_tiles();
    }

    /// Swap the current element with the next element.
    pub fn swap_with_next_element(&mut self) {
        if self.tiles.len() < 2 {
            return;
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

    /// Refresh the geometries of the tiles contained in this [`Workspace`].
    ///
    /// This ensures geometry for maximized and tiled elements.
    #[profiling::function]
    pub fn arrange_tiles(&mut self) {
        if self.tiles.is_empty() {
            return;
        }

        let layout = self.get_active_layout();
        let (maximized, tiled) = self
            .tiles
            .iter_mut()
            .partition::<Vec<_>, _>(|tile| tile.element.maximized());

        let inner_gaps = CONFIG.general.inner_gaps;
        let outer_gaps = CONFIG.general.outer_gaps;

        let usable_geo = layer_map_for_output(&self.output)
            .non_exclusive_zone()
            .as_local();
        let mut maximized_geo = usable_geo;
        maximized_geo.size -= (2 * outer_gaps, 2 * outer_gaps).into();
        maximized_geo.loc += (outer_gaps, outer_gaps).into();
        for tile in maximized {
            tile.set_geometry(maximized_geo)
        }

        if tiled.is_empty() {
            return;
        }

        let tiled_len = tiled.len();
        layout.arrange_tiles(tiled.into_iter(), tiled_len, maximized_geo, inner_gaps);
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

        {
            let ipc_path = self.ipc_path.clone();
            let layout = self.layouts[self.active_layout_idx].to_string();
            spawn(async move {
                let iface_ref = DBUS_CONNECTION
                    .object_server()
                    .inner()
                    .interface::<_, IpcWorkspace>(ipc_path.as_ref())
                    .await
                    .unwrap();
                let mut iface = iface_ref.get_mut().await;
                iface.active_layout = layout;
                iface
                    .active_layout_changed(iface_ref.signal_context())
                    .await
                    .unwrap();
            });
        }

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

        {
            let layout = self.layouts[self.active_layout_idx].to_string();
            let ipc_path = self.ipc_path.clone();
            spawn(async move {
                let iface_ref = DBUS_CONNECTION
                    .object_server()
                    .inner()
                    .interface::<_, IpcWorkspace>(ipc_path.as_ref())
                    .await
                    .unwrap();
                let mut iface = iface_ref.get_mut().await;
                iface.active_layout = layout;
                iface
                    .active_layout_changed(iface_ref.signal_context())
                    .await
                    .unwrap();
            });
        }

        self.active_layout_idx = new_active_idx;
        self.arrange_tiles();
    }

    /// Change the master_width_factor of the active [`WorkspaceLayout`]
    ///
    /// This clamps the value between (0.0..=0.95).
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
            *master_width_factor = master_width_factor.clamp(0.0, 0.95);
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
    pub fn tiles_under(&self, point: Point<f64, Global>) -> impl Iterator<Item = &WorkspaceTile<E>> {
        let point = point.to_local(&self.output);
        self.tiles
            .iter()
            .filter(move |tile| {
                if !tile.bbox().to_f64().contains(point) {
                    return false;
                }

                let render_location = tile.render_location();
                tile.element.is_in_input_region(&(point - render_location.to_f64()).as_logical())
            })
            // .filter(|tile| {
            //     let render_location = tile.render_location();
            //     if tile
            //         .element
            //         .is_in_input_region(&(point - render_location.to_f64()).as_logical())
            //     {
            //         Some((tile.element(), render_location.to_global(&self.output)))
            //     } else {
            //         None
            //     }
            // })
    }

    /// Render all elements in this [`Workspace`], respecting the window's Z-index.
    #[profiling::function]
    pub fn render_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<WorkspaceTileRenderElement<R>> {
        let mut above_render_elements = vec![];
        let render_elements: Vec<_> = self.tiles
            .iter()
            .enumerate()
            .filter_map(|(idx, tile)| {
                if tile.draw_above_others() {
                    above_render_elements = tile.render_elements(renderer, scale, alpha, true);
                    None
                } else {
                    Some(tile.render_elements(renderer, scale, alpha, idx == self.focused_tile_idx))
                }

            })
            .flatten()
            .collect();

        above_render_elements.extend(render_elements);
        above_render_elements
    }
}

// #[derive(Debug)]
// pub struct FullscreenSurface {
//     pub inner: E,
//     pub last_known_idx: usize,
// }
//
// impl PartialEq for FullscreenSurface {
//     fn eq(&self, other: &Self) -> bool {
//         &self.inner == &other.inner
//     }
// }

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

impl State {
    #[profiling::function]
    fn handle_workspace_ipc_request(&mut self, ipc_path: &str, req: IpcWorkspaceRequest) {
        let wset = self
            .fht
            .workspaces
            .values_mut()
            .find(|wset| wset.workspaces().any(|ws| ws.ipc_path.as_ref() == ipc_path))
            .unwrap();
        let active_idx = wset.get_active_idx();
        let (idx, workspace) = wset
            .workspaces_mut()
            .enumerate()
            .find(|(_, ws)| ws.ipc_path.as_ref() == ipc_path)
            .unwrap();
        let is_active = active_idx == idx;

        match req {
            IpcWorkspaceRequest::ChangeNmaster { delta } => workspace.change_nmaster(delta),
            IpcWorkspaceRequest::ChangeMasterWidthFactor { delta } => {
                workspace.change_mwfact(delta)
            }
            IpcWorkspaceRequest::SelectNextLayout => workspace.select_next_layout(),
            IpcWorkspaceRequest::SelectPreviousLayout => workspace.select_next_layout(),
            IpcWorkspaceRequest::FocusNextWindow => {
                let new_focus = workspace.focus_next_element().cloned();
                if is_active && let Some(window) = new_focus {
                    if CONFIG.general.cursor_warps {
                        let center = workspace.element_location(&window).unwrap();
                        self.move_pointer(center.to_f64())
                    }
                    self.set_focus_target(Some(window.into()));
                }
            }
            IpcWorkspaceRequest::FocusPreviousWindow => {
                let new_focus = workspace.focus_previous_element().cloned();
                if is_active && let Some(window) = new_focus {
                    if CONFIG.general.cursor_warps {
                        let center = workspace.element_location(&window).unwrap();
                        self.move_pointer(center.to_f64())
                    }
                    self.set_focus_target(Some(window.into()));
                }
            }
        }
    }
}
