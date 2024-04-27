use std::cmp::min;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_std::task::spawn;
use serde::{Deserialize, Serialize};
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::backend::renderer::element::{Element, RenderElement};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::DamageSet;
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::desktop::space::SpaceElement;
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::calloop::{self, LoopHandle, RegistrationToken};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Physical, Point, Rectangle, Scale};
use smithay::wayland::compositor::send_surface_state;

use super::window::FhtWindowRenderElement;
use super::FhtWindow;
use crate::backend::render::AsGlowRenderer;
#[cfg(feature = "udev_backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};
use crate::config::{WorkspaceSwitchAnimationDirection, CONFIG};
use crate::ipc::{IpcOutput, IpcWorkspace, IpcWorkspaceRequest};
use crate::state::State;
use crate::utils::animation::Animation;
use crate::utils::dbus::DBUS_CONNECTION;
use crate::utils::geometry::{
    Global, PointGlobalExt, RectCenterExt, RectExt, RectGlobalExt, RectLocalExt, SizeExt,
};
use crate::utils::output::OutputExt;

pub struct WorkspaceSet {
    /// The output of this set.
    pub(super) output: Output,

    /// All the workspaces of this set.
    pub workspaces: Vec<Workspace>,

    /// The current switch animation, of any.
    pub switch_animation: Option<WorkspaceSwitchAnimation>,

    /// The active workspace index.
    pub(super) active_idx: AtomicUsize,
}

#[allow(dead_code)]
impl WorkspaceSet {
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
    pub fn set_active_idx(&mut self, target_idx: usize, animate: bool) -> Option<FhtWindow> {
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
    pub fn active(&self) -> &Workspace {
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
    pub fn active_mut(&mut self) -> &mut Workspace {
        if let Some(WorkspaceSwitchAnimation { target_idx, .. }) = self.switch_animation.as_ref() {
            &mut self.workspaces[*target_idx]
        } else {
            &mut self.workspaces[self.active_idx.load(Ordering::SeqCst)]
        }
    }

    /// Get an iterator over all the [`Workspace`]s in this [`WorkspaceSet`]
    pub fn workspaces(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces.iter()
    }

    /// Get a mutable iterator over all the [`Workspace`]s in this [`WorkspaceSet`]
    pub fn workspaces_mut(&mut self) -> impl Iterator<Item = &mut Workspace> {
        self.workspaces.iter_mut()
    }

    /// Arrange the [`Workspace`]s and their windows.
    ///
    /// You need to call this when this [`WorkspaceSet`] output changes geometry to ensure that
    /// the tiled window geometries actually fill the output space.
    pub fn arrange(&self) {
        self.workspaces()
            .for_each(Workspace::refresh_window_geometries)
    }

    /// Find the window associated with this [`WlSurface`]
    pub fn find_window(&self, surface: &WlSurface) -> Option<&FhtWindow> {
        self.workspaces().find_map(|ws| ws.find_window(surface))
    }

    /// Find the workspace containing the window associated with this [`WlSurface`].
    pub fn find_workspace(&self, surface: &WlSurface) -> Option<&Workspace> {
        self.workspaces().find(|ws| ws.has_surface(surface))
    }

    /// Find the workspace containing the window associated with this [`WlSurface`].
    pub fn find_workspace_mut(&mut self, surface: &WlSurface) -> Option<&mut Workspace> {
        self.workspaces_mut().find(|ws| ws.has_surface(surface))
    }

    /// Find the window associated with this [`WlSurface`] with the [`Workspace`] containing it.
    pub fn find_window_and_workspace(
        &self,
        surface: &WlSurface,
    ) -> Option<(&FhtWindow, &Workspace)> {
        self.workspaces()
            .find_map(|ws| ws.find_window(surface).map(|w| (w, ws)))
    }

    /// Find the window associated with this [`WlSurface`] with the [`Workspace`] containing it.
    pub fn find_window_and_workspace_mut(
        &mut self,
        surface: &WlSurface,
    ) -> Option<(FhtWindow, &mut Workspace)> {
        self.workspaces_mut()
            .find_map(|ws| ws.find_window(surface).cloned().map(|w| (w, ws)))
    }

    /// Get a reference to the [`Workspace`] holding this window, if any.
    pub fn ws_for(&self, window: &FhtWindow) -> Option<&Workspace> {
        self.workspaces().find(|ws| ws.has_window(window))
    }

    /// Get a mutable reference to the [`Workspace`] holding this window, if any.
    pub fn ws_mut_for(&mut self, window: &FhtWindow) -> Option<&mut Workspace> {
        self.workspaces_mut().find(|ws| ws.has_window(window))
    }

    /// Get the current fullscreen window and it's location in global coordinate space.
    ///
    /// This function also accounts for workspace switch animations.
    #[profiling::function]
    pub fn current_fullscreen(&self) -> Option<(&FhtWindow, Point<i32, Global>)> {
        if self.switch_animation.is_none() {
            // It's just the active one, so no need to do additional calculations.
            return self
                .active()
                .fullscreen
                .as_ref()
                .map(|f| (&f.inner, f.inner.render_location()));
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
            .fullscreen
            .as_ref()
            .map(|f| (&f.inner, f.inner.render_location() + current_offset))
            .or_else(|| {
                self.workspaces[animation.target_idx]
                    .fullscreen
                    .as_ref()
                    .map(|f| (&f.inner, f.inner.render_location() + target_offset))
            })
    }

    /// Get the window in under the cursor and it's location in global coordinate space.
    ///
    /// This function also accounts for workspace switch animations.
    #[profiling::function]
    pub fn window_under(
        &self,
        point: Point<f64, Global>,
    ) -> Option<(&FhtWindow, Point<i32, Global>)> {
        if self.switch_animation.is_none() {
            // It's just the active one, so no need to do additional calculations.
            return self.active().window_under(point);
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
            .window_under(point + current_offset.to_f64())
            .map(|(ft, loc)| (ft, loc + current_offset))
            .or_else(|| {
                self.workspaces[animation.target_idx]
                    .window_under(point + target_offset.to_f64())
                    .map(|(ft, loc)| (ft, loc + target_offset))
            })
    }

    /// Render all the elements in this workspace set, returning them and whether it currently
    /// holds a fullscreen window.
    #[profiling::function]
    pub fn render_elements<R>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
        alpha: f32,
    ) -> (bool, Vec<WorkspaceSetRenderElement<R>>)
    where
        R: Renderer + ImportAll + ImportMem + AsGlowRenderer,
        <R as Renderer>::TextureId: Clone + 'static,

        FhtWindowRenderElement<R>: RenderElement<R>,
        WaylandSurfaceRenderElement<R>: RenderElement<R>,
    {
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

            return (active.fullscreen.is_some(), elements);
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
            return (target.fullscreen.is_some(), elements);
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
            active.fullscreen.is_some() || target.fullscreen.is_some(),
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

#[derive(Debug)]
pub enum WorkspaceSetRenderElement<R>
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: 'static,

    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
{
    Normal(FhtWindowRenderElement<R>),
    // FIXME: This makes the border look funky. Should go figure out why
    // Switching(CropRenderElement<RelocateRenderElement<FhtWindowRenderElement<R>>>),
    Switching(RelocateRenderElement<FhtWindowRenderElement<R>>),
}

impl<R> Element for WorkspaceSetRenderElement<R>
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: 'static,

    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
{
    fn id(&self) -> &smithay::backend::renderer::element::Id {
        match self {
            Self::Normal(e) => e.id(),
            Self::Switching(e) => e.id(),
        }
    }

    fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
        match self {
            Self::Normal(e) => e.current_commit(),
            Self::Switching(e) => e.current_commit(),
        }
    }

    fn src(&self) -> Rectangle<f64, smithay::utils::Buffer> {
        match self {
            Self::Normal(e) => e.src(),
            Self::Switching(e) => e.src(),
        }
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, smithay::utils::Physical> {
        match self {
            Self::Normal(e) => e.geometry(scale),
            Self::Switching(e) => e.geometry(scale),
        }
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, smithay::utils::Physical> {
        match self {
            Self::Normal(e) => e.location(scale),
            Self::Switching(e) => e.location(scale),
        }
    }

    fn transform(&self) -> smithay::utils::Transform {
        match self {
            Self::Normal(e) => e.transform(),
            Self::Switching(e) => e.transform(),
        }
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<smithay::backend::renderer::utils::CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        match self {
            Self::Normal(e) => e.damage_since(scale, commit),
            Self::Switching(e) => e.damage_since(scale, commit),
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, smithay::utils::Physical>> {
        match self {
            Self::Normal(e) => e.opaque_regions(scale),
            Self::Switching(e) => e.opaque_regions(scale),
        }
    }

    fn alpha(&self) -> f32 {
        match self {
            Self::Normal(e) => e.alpha(),
            Self::Switching(e) => e.alpha(),
        }
    }

    fn kind(&self) -> smithay::backend::renderer::element::Kind {
        match self {
            Self::Normal(e) => e.kind(),
            Self::Switching(e) => e.kind(),
        }
    }
}

impl RenderElement<GlowRenderer> for WorkspaceSetRenderElement<GlowRenderer> {
    fn draw(
        &self,
        frame: &mut GlowFrame,
        src: Rectangle<f64, smithay::utils::Buffer>,
        dst: Rectangle<i32, smithay::utils::Physical>,
        damage: &[Rectangle<i32, smithay::utils::Physical>],
    ) -> Result<(), <GlowRenderer as Renderer>::Error> {
        match self {
            Self::Normal(e) => e.draw(frame, src, dst, damage),
            Self::Switching(e) => e.draw(frame, src, dst, damage),
        }
    }

    fn underlying_storage(
        &self,
        renderer: &mut GlowRenderer,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage> {
        match self {
            Self::Normal(e) => e.underlying_storage(renderer),
            Self::Switching(e) => e.underlying_storage(renderer),
        }
    }
}

#[cfg(feature = "udev_backend")]
impl<'a> RenderElement<UdevRenderer<'a>> for WorkspaceSetRenderElement<UdevRenderer<'a>> {
    fn draw(
        &self,
        frame: &mut UdevFrame<'a, '_>,
        src: Rectangle<f64, smithay::utils::Buffer>,
        dst: Rectangle<i32, smithay::utils::Physical>,
        damage: &[Rectangle<i32, smithay::utils::Physical>],
    ) -> Result<(), UdevRenderError<'a>> {
        match self {
            Self::Normal(e) => e.draw(frame, src, dst, damage),
            Self::Switching(e) => e.draw(frame, src, dst, damage),
        }
    }

    fn underlying_storage(
        &self,
        renderer: &mut UdevRenderer<'a>,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage> {
        match self {
            Self::Normal(e) => e.underlying_storage(renderer),
            Self::Switching(e) => e.underlying_storage(renderer),
        }
    }
}

/// A single workspace.
///
/// This workspace should not stand on it's own, and it's preferred you use it with a
/// [`WorkspaceSet`], but nothing stops you from doing whatever you want with it like assigning it
/// to a single output.
#[derive(Debug)]
pub struct Workspace {
    /// The output for this workspace
    output: Output,

    /// The window this workspace contains.
    ///
    /// These must all have valid [`WlSurface`]s (aka: being mapped), otherwise the workspace inner
    /// logic will PANIC.
    ///
    /// WARNING: We shouldn't expose this to keep the dbus interface in sync, but here its symbol
    /// to drain the windows when deleting an output, soo it should be fine
    pub windows: Vec<FhtWindow>,

    /// The focused window index.
    focused_window_idx: usize,

    /// The currently fullscreened window, if any.
    ///
    /// How [`Workspace`]s handle fullscreening is a bit "weird" and "unconventional":
    /// Only one window per workspace can be fullscreened at a time.
    ///
    /// When that window is fullscreened, it's removed from the window list so that it can be
    /// rendered exclusively on this workspace, so that we can profit from direct scan-out of
    /// fullscreen window, very useful for game performance.
    ///
    /// Doing actions such as using focus_next_window/focus_previous_window will remove the
    /// fullscreen and insert it back at the last index it was at.
    pub fullscreen: Option<FullscreenSurface>,

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

impl Drop for Workspace {
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

impl Workspace {
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

            windows: vec![],
            fullscreen: None,
            focused_window_idx: 0,

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
        if let Some(FullscreenSurface {
            inner,
            mut last_known_idx,
        }) = self
            .fullscreen
            .take_if(|f| !f.inner.alive() || !f.inner.fullscreen())
        {
            should_refresh_geometries = true;
            inner.set_fullscreen(false, None);
            last_known_idx = last_known_idx.clamp(0, self.windows.len());
            // NOTE: I assume that if you call this function you don't have a handle to the inner
            // fullscreen window, so just make sure it understood theres no more fullscreen.
            inner.set_fullscreen(false, None);
            inner.toplevel().send_pending_configure();

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
                    iface.fullscreen = None;
                    iface
                        .fullscreen_changed(iface_ref.signal_context())
                        .await
                        .unwrap();
                });
            }

            self.windows.insert(last_known_idx, inner);
        }

        // Clean dead/zombie windows
        // Also ensure that we dont try to access out of bounds indexes, and sync up the IPC.
        let mut removed_ids = vec![];
        self.windows.retain(|window| {
            if !window.alive() {
                removed_ids.push(window.uid());
                false
            } else {
                true
            }
        });
        let new_len = self.windows.len();
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
            self.focused_window_idx = self.focused_window_idx.clamp(0, new_len.saturating_sub(1));
            self.refresh_window_geometries();
        }

        // Refresh internal state of windows
        //
        if let Some(FullscreenSurface { inner, .. }) = self.fullscreen.as_ref() {
            inner.set_activated(true);
            inner.surface.refresh();
        }
        let output_geometry = self.output.geometry();
        for window in self.windows.iter() {
            // This is now managed globally with focus targets
            // window.set_activated(idx == self.focused_window_idx);

            let bbox = window.bbox();
            if let Some(mut overlap) = output_geometry.intersection(bbox) {
                // output_enter excepts the overlap to be relative to the element, weird choice but
                // I comply.
                overlap.loc -= bbox.loc;
                window
                    .surface
                    .output_enter(&self.output, overlap.as_logical());
            }

            window.surface.refresh();
        }
    }

    /// Return whether this workspace has this window.
    pub fn find_window(&self, surface: &WlSurface) -> Option<&FhtWindow> {
        self.fullscreen
            .as_ref()
            .filter(|f| f.inner.wl_surface() == *surface)
            .map(|f| &f.inner)
            .or_else(|| self.windows.iter().find(|w| w.wl_surface() == *surface))
    }

    /// Return whether this workspace has this window.
    pub fn has_window(&self, window: &FhtWindow) -> bool {
        self.fullscreen.as_ref().is_some_and(|f| f.inner == *window)
            || self.windows.iter().any(|w| w == window)
    }

    /// Return whether this workspace has a window with this [`WlSurface`] as its toplevel surface.
    pub fn has_surface(&self, surface: &WlSurface) -> bool {
        self.fullscreen
            .as_ref()
            .is_some_and(|f| f.inner.has_surface(surface, WindowSurfaceType::TOPLEVEL))
            || self
                .windows
                .iter()
                .any(|w| w.has_surface(surface, WindowSurfaceType::TOPLEVEL))
    }

    /// Return the focused window, giving priority to the fullscreen window first, then the
    /// possible active non-fullscreen window.
    pub fn focused(&self) -> Option<&FhtWindow> {
        self.fullscreen
            .as_ref()
            .map(|f| &f.inner)
            .or_else(|| self.windows.get(self.focused_window_idx))
    }

    /// Insert a window in this [`Workspace`]
    ///
    /// This function does additional configuration of the window before inserting it in the window
    /// list, mainly setting the bounds of the window, and notifying it of entering this
    /// [`Workspace`] output.
    ///
    /// This doesn't reinsert a window if it's already inserted.
    pub fn insert_window(&mut self, window: FhtWindow) {
        if self.windows.contains(&window)
            || self.fullscreen.as_ref().is_some_and(|f| f.inner == window)
        {
            return;
        }

        if let Some(fullscreen) = self.remove_current_fullscreen() {
            fullscreen.set_fullscreen(false, None);
            fullscreen.surface.toplevel().send_pending_configure();
        }

        // Configure the window for insertion
        // refresh_window_geometries send a configure message for us
        window.surface.output_enter(
            &self.output,
            window.bbox().to_local(&self.output).as_logical(),
        );
        window.set_bounds(Some(self.output.geometry().size.as_logical()));
        // configure the wl_surface
        let scale = self.output.current_scale().integer_scale();
        let transform = self.output.current_transform();
        window.with_surfaces(|surface, data| send_surface_state(surface, data, scale, transform));

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

        self.windows.push(window.clone());
        if window.fullscreen() {
            self.fullscreen_window(&window);
        } else {
            if CONFIG.general.focus_new_windows {
                self.focused_window_idx = self.windows.len() - 1;
            }
            self.refresh_window_geometries();
        }
    }

    /// Removes a window from this [`Workspace`], returning it if it was found.
    ///
    /// This function also undones the configuration that was done in [`Self::insert_window`]
    pub fn remove_window(&mut self, window: &FhtWindow) -> Option<FhtWindow> {
        if let Some(fullscreen) = self.fullscreen.take_if(|f| &f.inner == window) {
            return Some(fullscreen.inner);
        }

        let Some(idx) = self.windows.iter().position(|w| w == window) else {
            return None;
        };

        let window = self.windows.remove(idx);
        // "Un"-configure the window (for potentially inserting it on another workspace who knows)
        window.surface.output_leave(&self.output);
        window.set_bounds(None);
        self.focused_window_idx = self.focused_window_idx.clamp(0, self.windows.len() - 1);

        {
            let ipc_path = self.ipc_path.clone();
            let window_id = window.uid();
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

        self.refresh_window_geometries();
        Some(window)
    }

    /// Focus a given window, if this [`Workspace`] contains it.
    pub fn focus_window(&mut self, window: &FhtWindow) {
        if let Some(idx) = self.windows.iter().position(|w| w == window) {
            self.focused_window_idx = idx;

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

    /// Focus the next available window, cycling back to the first one if needed.
    pub fn focus_next_window(&mut self) -> Option<&FhtWindow> {
        if self.windows.is_empty() {
            return None;
        }

        if let Some(fullscreen) = self.remove_current_fullscreen() {
            fullscreen.set_fullscreen(false, None);
            // refresh window geos will send a configure req for us.
            self.refresh_window_geometries();
        }

        let windows_len = self.windows.len();
        let new_focused_idx = self.focused_window_idx + 1;
        self.focused_window_idx = if new_focused_idx == windows_len {
            0
        } else {
            new_focused_idx
        };

        {
            let ipc_path = self.ipc_path.clone();
            let focused_window_idx = self.focused_window_idx as u8;
            spawn(async move {
                let iface_ref = DBUS_CONNECTION
                    .object_server()
                    .inner()
                    .interface::<_, IpcWorkspace>(ipc_path.as_ref())
                    .await
                    .unwrap();
                let mut iface = iface_ref.get_mut().await;
                iface.focused_window_index = focused_window_idx;
                iface
                    .focused_window_changed(iface_ref.signal_context())
                    .await
                    .unwrap();
            });
        }

        let window = &self.windows[self.focused_window_idx];
        self.raise_window(window);
        Some(window)
    }

    /// Focus the previous available window, cyclying all the way to the last window if needed.
    pub fn focus_previous_window(&mut self) -> Option<&FhtWindow> {
        if self.windows.is_empty() {
            return None;
        }

        if let Some(fullscreen) = self.remove_current_fullscreen() {
            fullscreen.set_fullscreen(false, None);
            // refresh window geos will send a configure req for us.
            self.refresh_window_geometries();
        }

        let windows_len = self.windows.len();
        self.focused_window_idx = match self.focused_window_idx.checked_sub(1) {
            Some(idx) => idx,
            None => windows_len - 1,
        };

        {
            let ipc_path = self.ipc_path.clone();
            let focused_window_idx = self.focused_window_idx as u8;
            spawn(async move {
                let iface_ref = DBUS_CONNECTION
                    .object_server()
                    .inner()
                    .interface::<_, IpcWorkspace>(ipc_path.as_ref())
                    .await
                    .unwrap();
                let mut iface = iface_ref.get_mut().await;
                iface.focused_window_index = focused_window_idx;
                iface
                    .focused_window_changed(iface_ref.signal_context())
                    .await
                    .unwrap();
            });
        }

        let window = &self.windows[self.focused_window_idx];
        self.raise_window(window);
        Some(window)
    }

    /// Swap the current window with the next window.
    pub fn swap_with_next_window(&mut self) {
        if self.windows.len() < 2 {
            return;
        }

        let windows_len = self.windows.len();
        let last_focused_idx = self.focused_window_idx;

        let new_focused_idx = self.focused_window_idx + 1;
        let new_focused_idx = if new_focused_idx == windows_len {
            0
        } else {
            new_focused_idx
        };

        self.focused_window_idx = new_focused_idx;
        self.windows.swap(last_focused_idx, new_focused_idx);
        self.refresh_window_geometries();
    }

    /// Swap the current window with the previous window.
    pub fn swap_with_previous_window(&mut self) {
        if self.windows.len() < 2 {
            return;
        }

        let windows_len = self.windows.len();
        let last_focused_idx = self.focused_window_idx;

        let new_focused_idx = match self.focused_window_idx.checked_sub(1) {
            Some(idx) => idx,
            None => windows_len - 1,
        };

        self.focused_window_idx = new_focused_idx;
        self.windows.swap(last_focused_idx, new_focused_idx);
        self.refresh_window_geometries();
    }

    /// Fullscreen a given window, if this [`Workspace`] contains it.
    ///
    /// NOTE: You still have to configure the window for it to know that it's fullscreened.
    pub fn fullscreen_window(&mut self, window: &FhtWindow) {
        let Some(idx) = self.windows.iter().position(|w| w == window) else {
            return;
        };

        let window = self.windows.remove(idx);

        {
            let window_uid = window.uid();
            let ipc_path = self.ipc_path.clone();
            spawn(async move {
                let iface_ref = DBUS_CONNECTION
                    .object_server()
                    .inner()
                    .interface::<_, IpcWorkspace>(ipc_path.as_ref())
                    .await
                    .unwrap();
                let mut iface = iface_ref.get_mut().await;
                iface.fullscreen = Some(window_uid);
                iface
                    .fullscreen_changed(iface_ref.signal_context())
                    .await
                    .unwrap();
                iface.windows.retain(|uid| *uid != window_uid);
                iface
                    .windows_changed(iface_ref.signal_context())
                    .await
                    .unwrap();
            });
        }

        self.fullscreen = Some(FullscreenSurface {
            inner: window,
            last_known_idx: idx,
        });

        self.focused_window_idx = self.focused_window_idx.saturating_sub(1);
        self.refresh_window_geometries();
    }

    /// Remove the current fullscreened window, if any.
    ///
    /// NOTE: You still have to configure the window for it to know that it's not fullscreened
    /// anymore.
    pub fn remove_current_fullscreen(&mut self) -> Option<&FhtWindow> {
        let FullscreenSurface {
            inner,
            mut last_known_idx,
        } = self.fullscreen.take()?;
        last_known_idx = last_known_idx.clamp(0, self.windows.len());
        let window_uid = inner.uid();
        self.windows.insert(last_known_idx, inner);
        self.focused_window_idx = last_known_idx;
        self.refresh_window_geometries();

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
                iface.fullscreen = None;
                iface
                    .fullscreen_changed(iface_ref.signal_context())
                    .await
                    .unwrap();
                iface.windows.insert(last_known_idx, window_uid);
                iface
                    .windows_changed(iface_ref.signal_context())
                    .await
                    .unwrap();
            });
        }

        Some(&self.windows[last_known_idx])
    }

    /// Refresh the geometries of the windows contained in this [`Workspace`].
    ///
    /// This assures the fullscreen windows take the full output geometry, maximized use
    /// non-exclusive layer shell areas, and arrange the rest of the tiled windows based on the
    /// active workspace layout.
    #[profiling::function]
    pub fn refresh_window_geometries(&self) {
        if let Some(window) = self.fullscreen.as_ref().map(|f| &f.inner) {
            window.set_geometry(self.output.geometry(), false);
            window.toplevel().send_pending_configure();
        }

        if self.windows.is_empty() {
            return;
        }

        let (maximized_windows, mut tiled_windows): (Vec<&FhtWindow>, Vec<&FhtWindow>) =
            self.windows.iter().partition(|w| w.maximized());
        tiled_windows.retain(|w| w.tiled());

        let inner_gaps = CONFIG.general.inner_gaps;
        let outer_gaps = CONFIG.general.outer_gaps;

        let usable_geo = layer_map_for_output(&self.output)
            .non_exclusive_zone()
            .as_local()
            .to_global(&self.output);
        let mut maximized_geo = usable_geo;
        maximized_geo.size -= (2 * outer_gaps, 2 * outer_gaps).into();
        maximized_geo.loc += (outer_gaps, outer_gaps).into();
        for window in maximized_windows {
            window.set_geometry_with_border(maximized_geo, false);
            window.toplevel().send_pending_configure();
        }

        if !tiled_windows.is_empty() {
            let windows_len = tiled_windows.len();
            self.get_active_layout().tile_windows(
                tiled_windows.into_iter(),
                windows_len,
                maximized_geo,
                inner_gaps,
                |_idx, w, new_geo| {
                    w.set_geometry_with_border(new_geo, false);
                    w.toplevel().send_pending_configure();
                },
            );
        }
    }

    /// Get the active layout that windows use for tiling.
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

        self.refresh_window_geometries();
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
        self.refresh_window_geometries();
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
        self.refresh_window_geometries();
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
        self.refresh_window_geometries();
    }

    /// Get the window under the pointer in this workspace.
    #[profiling::function]
    pub fn window_under(
        &self,
        point: Point<f64, Global>,
    ) -> Option<(&FhtWindow, Point<i32, Global>)> {
        if let Some(FullscreenSurface { inner, .. }) = self.fullscreen.as_ref() {
            return Some((inner, inner.render_location()));
        }

        let mut windows = self.windows.iter().collect::<Vec<_>>();
        windows.sort_by_key(|w| std::cmp::Reverse(w.z_index()));

        windows
            .iter()
            .filter(|w| w.bbox().to_f64().contains(point))
            .find_map(|w| {
                let render_location = w.render_location();
                if w.surface
                    .is_in_input_region(&(point - render_location.to_f64()).as_logical())
                {
                    Some((*w, render_location))
                } else {
                    None
                }
            })
    }

    /// Raise the given window above all other windows, if found.
    #[profiling::function]
    pub fn raise_window(&self, window: &FhtWindow) {
        if !self.windows.contains(window) {
            return;
        }

        let old_z_index = window.z_index();
        let max_z_index = self.windows.iter().map(FhtWindow::z_index).sum::<u32>();
        if old_z_index <= max_z_index {
            window.set_z_index(max_z_index + 1);
        }
    }

    /// Render all elements in this [`Workspace`], respecting the window's Z-index.
    #[profiling::function]
    pub fn render_elements<R>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<FhtWindowRenderElement<R>>
    where
        R: Renderer + ImportAll + ImportMem + AsGlowRenderer,
        <R as Renderer>::TextureId: Clone + 'static,

        FhtWindowRenderElement<R>: RenderElement<R>,
        WaylandSurfaceRenderElement<R>: RenderElement<R>,
    {
        if let Some(FullscreenSurface { inner, .. }) = self.fullscreen.as_ref() {
            return inner.render_elements(renderer, scale, alpha);
        }

        let mut windows = self.windows.iter().collect::<Vec<_>>();
        windows.sort_unstable_by(|a, b| a.z_index().cmp(&b.z_index()));
        windows.reverse();

        windows
            .into_iter()
            .flat_map(|w| w.render_elements(renderer, scale, alpha))
            .collect()
    }
}

#[derive(Debug)]
pub struct FullscreenSurface {
    pub inner: FhtWindow,
    pub last_known_idx: usize,
}

impl PartialEq for FullscreenSurface {
    fn eq(&self, other: &Self) -> bool {
        &self.inner == &other.inner
    }
}

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

impl WorkspaceLayout {
    /// Tile `windows` inside `tile_area` while letting `inner_gaps` between them.
    ///
    /// You can decide on how you apply the geometry to the window in the apply_geometry closure
    pub fn tile_windows<'a>(
        &'a self,
        windows: impl Iterator<Item = &'a FhtWindow>,
        windows_len: usize,
        tile_area: Rectangle<i32, Global>,
        inner_gaps: i32,
        apply_geometry: impl Fn(usize, &FhtWindow, Rectangle<i32, Global>),
    ) {
        match *self {
            WorkspaceLayout::Tile {
                nmaster,
                master_width_factor,
            } => {
                // A lone master window in a workspace will basically appear the same as a
                // maximized window, so it's logical to start from there
                let master_len = min(windows_len, nmaster);
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
                let stack_len = windows_len.saturating_sub(nmaster);
                let mut stack_geo = tile_area;
                let mut stack_rest = 0;
                stack_geo.size.h -= inner_gaps * stack_len.saturating_sub(1) as i32;
                if windows_len > nmaster {
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

                for (idx, window) in windows.enumerate() {
                    if idx < nmaster {
                        let mut master_height = master_geo.size.h;
                        if master_rest != 0 {
                            master_height += 1;
                            master_rest -= 1;
                        }

                        apply_geometry(
                            idx,
                            window,
                            Rectangle::from_loc_and_size(
                                master_geo.loc,
                                (master_geo.size.w, master_height),
                            ),
                        );

                        master_geo.loc.y += master_height + inner_gaps;
                    } else {
                        let mut stack_height = stack_geo.size.h;
                        if stack_rest != 0 {
                            stack_height += 1;
                            stack_rest -= 1;
                        }

                        apply_geometry(
                            idx,
                            window,
                            Rectangle::from_loc_and_size(
                                stack_geo.loc,
                                (stack_geo.size.w, stack_height),
                            ),
                        );

                        stack_geo.loc.y += stack_height + inner_gaps;
                    }

                    window.toplevel().send_pending_configure();
                }
            }
            WorkspaceLayout::BottomStack {
                nmaster,
                master_width_factor,
            } => {
                // A lone master window in a workspace will basically appear the same as a
                // maximized window, so it's logical to start from there
                let master_len = min(windows_len, nmaster);
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
                let stack_len = windows_len.saturating_sub(nmaster);
                let mut stack_geo = tile_area;
                let mut stack_rest = 0;
                stack_geo.size.w -= inner_gaps * stack_len.saturating_sub(1) as i32;
                if windows_len > nmaster {
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

                for (idx, window) in windows.enumerate() {
                    if idx < nmaster {
                        let mut master_width = master_geo.size.w;
                        if master_rest != 0 {
                            master_width += 1;
                            master_rest -= 1;
                        }

                        apply_geometry(
                            idx,
                            window,
                            Rectangle::from_loc_and_size(
                                master_geo.loc,
                                (master_width, master_geo.size.h),
                            ),
                        );

                        master_geo.loc.x += master_width + inner_gaps;
                    } else {
                        let mut stack_width = stack_geo.size.w;
                        if stack_rest != 0 {
                            stack_width += 1;
                            stack_rest -= 1;
                        }

                        apply_geometry(
                            idx,
                            window,
                            Rectangle::from_loc_and_size(
                                stack_geo.loc,
                                (stack_width, stack_geo.size.h),
                            ),
                        );

                        stack_geo.loc.x += stack_width + inner_gaps;
                    }

                    window.toplevel().send_pending_configure();
                }
            }
            #[allow(unused)]
            WorkspaceLayout::CenteredMaster {
                nmaster,
                master_width_factor,
            } => {
                // A lone master window in a workspace will basically appear the same as a
                // maximized window, so it's logical to start from there
                let master_len = min(windows_len, nmaster);
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
                let left_len = windows_len.saturating_sub(nmaster) / 2;
                let mut left_geo = Rectangle::default();
                left_geo.size.h =
                    tile_area.size.h - (inner_gaps * left_len.saturating_sub(1) as i32);
                left_geo.size.h = (left_geo.size.h as f64 / left_len as f64).floor() as i32;
                let mut left_rest = tile_area.size.h
                    - (left_len.saturating_sub(1) as i32 * inner_gaps)
                    - (left_len as i32 * left_geo.size.h);

                // Repeat again for right column
                let right_len = (windows_len.saturating_sub(nmaster) / 2) as i32
                    + (windows_len.saturating_sub(nmaster) % 2) as i32;
                let mut right_geo = Rectangle::default();
                right_geo.size.h =
                    tile_area.size.h - (inner_gaps * right_len.saturating_sub(1) as i32);
                right_geo.size.h = (right_geo.size.h as f64 / right_len as f64).floor() as i32;
                let mut right_rest = tile_area.size.h
                    - (right_len.saturating_sub(1) as i32 * inner_gaps)
                    - (right_len as i32 * right_geo.size.h);

                if windows_len > nmaster {
                    if (windows_len - nmaster) > 1 {
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

                for (idx, window) in windows.enumerate() {
                    if idx < nmaster {
                        let mut master_height = master_geo.size.h;
                        if master_rest != 0 {
                            master_height += 1;
                            master_rest -= 1;
                        }

                        apply_geometry(
                            idx,
                            window,
                            Rectangle::from_loc_and_size(
                                master_geo.loc,
                                (master_geo.size.w, master_height),
                            ),
                        );

                        master_geo.loc.y += master_geo.size.h + inner_gaps;
                    } else if ((idx - nmaster) % 2 != 0) {
                        let mut left_height = left_geo.size.h;
                        if left_rest != 0 {
                            left_height += 1;
                            left_rest -= 1;
                        }

                        apply_geometry(
                            idx,
                            window,
                            Rectangle::from_loc_and_size(
                                left_geo.loc,
                                (left_geo.size.w, left_height),
                            ),
                        );

                        left_geo.loc.y += left_geo.size.h + inner_gaps;
                    } else {
                        let mut right_height = right_geo.size.h;
                        if right_rest != 0 {
                            right_height += 1;
                            right_rest -= 1;
                        }

                        apply_geometry(
                            idx,
                            window,
                            Rectangle::from_loc_and_size(
                                right_geo.loc,
                                (right_geo.size.w, right_height),
                            ),
                        );

                        right_geo.loc.y += right_geo.size.h + inner_gaps;
                    }
                }
            }
            WorkspaceLayout::Floating => {
                // Let the windows be free
                for window in windows {
                    window.toplevel().send_pending_configure();
                }
            }
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
                let new_focus = workspace.focus_next_window().cloned();
                if is_active && let Some(window) = new_focus {
                    if CONFIG.general.cursor_warps {
                        let center = window.geometry().center();
                        self.move_pointer(center.to_f64())
                    }
                    self.set_focus_target(Some(window.into()));
                }
            }
            IpcWorkspaceRequest::FocusPreviousWindow => {
                let new_focus = workspace.focus_previous_window().cloned();
                if is_active && let Some(window) = new_focus {
                    if CONFIG.general.cursor_warps {
                        let center = window.geometry().center();
                        self.move_pointer(center.to_f64())
                    }
                    self.set_focus_target(Some(window.into()));
                }
            }
        }
    }
}
