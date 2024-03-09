use std::cmp::min;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::backend::renderer::element::{Element, RenderElement};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::DamageSet;
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::desktop::layer_map_for_output;
use smithay::desktop::space::SpaceElement;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Physical, Point, Rectangle, Scale};
use smithay::wayland::seat::WaylandFocus;

use super::window::FhtWindowRenderElement;
use super::FhtWindow;
use crate::backend::render::AsGlowRenderer;
#[cfg(feature = "udev_backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};
use crate::config::{WorkspaceSwitchAnimationDirection, CONFIG};
use crate::utils::animation::Animation;
use crate::utils::geometry::{
    Global, PointGlobalExt, RectExt, RectGlobalExt, RectLocalExt, SizeExt,
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
    pub fn new(output: Output) -> Self {
        Self {
            output: output.clone(),
            workspaces: (0..9).map(|_| Workspace::new(output.clone())).collect(),
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
        self.workspaces().find_map(|ws| {
            if let Some(FullscreenSurface { inner, .. }) = ws
                .fullscreen
                .as_ref()
                .filter(|f| f.inner.wl_surface().as_ref() == Some(surface))
            {
                Some(inner)
            } else {
                ws.windows
                    .iter()
                    .find(|w| w.wl_surface().as_ref() == Some(surface))
            }
        })
    }

    /// Find the workspace containing the window associated with this [`WlSurface`].
    pub fn find_workspace(&self, surface: &WlSurface) -> Option<&Workspace> {
        self.workspaces().find(|ws| {
            ws.windows
                .iter()
                .any(|w| w.wl_surface().as_ref() == Some(surface))
        })
    }

    /// Find the workspace containing the window associated with this [`WlSurface`].
    pub fn find_workspace_mut(&mut self, surface: &WlSurface) -> Option<&mut Workspace> {
        self.workspaces_mut().find(|ws| {
            ws.windows
                .iter()
                .any(|w| w.wl_surface().as_ref() == Some(surface))
        })
    }

    /// Find the window associated with this [`WlSurface`] with the [`Workspace`] containing it.
    pub fn find_window_and_workspace(
        &self,
        surface: &WlSurface,
    ) -> Option<(&FhtWindow, &Workspace)> {
        self.workspaces().find_map(|ws| {
            let window = ws
                .windows
                .iter()
                .find(|w| w.wl_surface().as_ref() == Some(surface));
            window.map(|w| (w, ws))
        })
    }

    /// Find the window associated with this [`WlSurface`] with the [`Workspace`] containing it.
    pub fn find_window_and_workspace_mut(
        &mut self,
        surface: &WlSurface,
    ) -> Option<(FhtWindow, &mut Workspace)> {
        self.workspaces_mut().find_map(|ws| {
            let window = ws
                .windows
                .iter()
                .find(|w| w.wl_surface().as_ref() == Some(surface))
                .cloned();
            window.map(|w| (w, ws))
        })
    }

    /// Get a reference to the [`Workspace`] holding this window, if any.
    pub fn ws_for(&self, window: &FhtWindow) -> Option<&Workspace> {
        self.workspaces()
            .find(|ws| ws.windows.iter().any(|w| w == window))
    }

    /// Get a mutable reference to the [`Workspace`] holding this window, if any.
    pub fn ws_mut_for(&mut self, window: &FhtWindow) -> Option<&mut Workspace> {
        self.workspaces_mut()
            .find(|ws| ws.windows.iter().any(|w| w == window))
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
        <R as Renderer>::TextureId: 'static,

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
            CONFIG.animation.workspace_switch.easing,
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
    R: Renderer + ImportAll + ImportMem + AsGlowRenderer,
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
    pub windows: Vec<FhtWindow>,

    /// The focused window index.
    pub focused_window_idx: usize,

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
    pub active_layout_idx: usize,
}

impl Workspace {
    /// Create a new [`Workspace`] for this output.
    pub fn new(output: Output) -> Self {
        Self {
            output,

            windows: vec![],
            fullscreen: None,
            focused_window_idx: 0,

            layouts: vec![
                WorkspaceLayout::BottomStack {
                    nmaster: 1,
                    master_width_factor: 0.5,
                },
                WorkspaceLayout::Tile {
                    nmaster: 1,
                    master_width_factor: 0.5,
                },
            ],
            active_layout_idx: 0,
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
            .take_if(|f| !f.inner.alive() || !f.inner.is_fullscreen())
        {
            should_refresh_geometries = true;
            inner.set_fullscreen(false, None);
            last_known_idx = last_known_idx.clamp(0, self.windows.len());
            // NOTE: I assume that if you call this function you don't have a handle to the inner
            // fullscreen window, so just make sure it understood theres no more fullscreen.
            inner.set_fullscreen(false, None);
            if let Some(toplevel) = inner.0.toplevel() {
                toplevel.send_pending_configure();
            }

            self.windows.insert(last_known_idx, inner);
        }

        // Clean dead/zombie windows
        // Also ensure that we dont try to access out of bounds indexes.
        let old_len = self.windows.len();
        self.windows.retain(FhtWindow::alive);
        let new_len = self.windows.len();
        if new_len != old_len {
            should_refresh_geometries = true;
        }

        if should_refresh_geometries {
            self.focused_window_idx = self.focused_window_idx.clamp(0, new_len.saturating_sub(1));
            self.refresh_window_geometries();
        }

        // Refresh internal state of windows
        let output_geometry = self.output.geometry();
        for (idx, window) in self.windows.iter().enumerate() {
            window.set_activate(idx == self.focused_window_idx);

            let bbox = window.global_bbox();
            if let Some(mut overlap) = output_geometry.intersection(bbox) {
                // output_enter excepts the overlap to be relative to the element, weird choice but
                // I comply.
                overlap.loc -= bbox.loc;
                window.output_enter(&self.output, overlap.as_logical());
            }

            window.refresh();
        }
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
        if self.windows.contains(&window) {
            return;
        }

        if let Some(fullscreen) = self.remove_current_fullscreen() {
            fullscreen.set_fullscreen(false, None);
            if let Some(toplevel) = fullscreen.0.toplevel() {
                toplevel.send_pending_configure();
            }
        }

        // Configure the window for insertion
        // refresh_window_geometries send a configure message for us
        window.output_enter(&self.output, window.bbox());
        window.set_bounds(Some(self.output.geometry().size.as_logical()));

        self.windows.push(window);
        if CONFIG.general.focus_new_windows {
            self.focused_window_idx = self.windows.len() - 1;
        }
        self.refresh_window_geometries();
    }

    /// Removes a window from this [`Workspace`], returning it if it was found.
    ///
    /// This function also undones the configuration that was done in [`Self::insert_window`]
    pub fn remove_window(&mut self, window: &FhtWindow) -> Option<FhtWindow> {
        let Some(idx) = self.windows.iter().position(|w| w == window) else {
            return None;
        };

        let window = self.windows.remove(idx);
        // "Un"-configure the window (for potentially inserting it on another workspace who knows)
        window.output_leave(&self.output);
        window.set_bounds(None);
        self.focused_window_idx = self.focused_window_idx.clamp(0, self.windows.len() - 1);

        self.refresh_window_geometries();
        Some(window)
    }

    /// Focus a given window, if this [`Workspace`] contains it.
    pub fn focus_window(&mut self, window: &FhtWindow) {
        if let Some(idx) = self.windows.iter().position(|w| w == window) {
            self.focused_window_idx = idx;
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

        let window = &self.windows[self.focused_window_idx];
        self.raise_window(window);
        Some(window)
    }

    /// Swap the current window with the next window.
    /// TODO: This DOES NOT work.
    pub fn swap_with_next_window(&mut self) {
        if self.windows.is_empty() {
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
    }

    /// Swap the current window with the previous window.
    /// TODO: This DOES NOT work.
    pub fn swap_with_previous_window(&mut self) {
        if self.windows.is_empty() {
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
    }

    /// Fullscreen a given window, if this [`Workspace`] contains it.
    ///
    /// NOTE: You still have to configure the window for it to know that it's fullscreened.
    pub fn fullscreen_window(&mut self, window: &FhtWindow) {
        let Some(idx) = self.windows.iter().position(|w| w == window) else {
            return;
        };

        let window = self.windows.remove(idx);
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
        self.windows.insert(last_known_idx, inner);
        Some(&self.windows[last_known_idx])
    }

    /// Refresh the geometries of the windows contained in this [`Workspace`].
    ///
    /// This assures the fullscreen windows take the full output geometry, maximized use
    /// non-exclusive layer shell areas, and arrange the rest of the tiled windows based on the
    /// active workspace layout.
    #[profiling::function]
    pub fn refresh_window_geometries(&self) {
        if self.windows.is_empty() {
            return;
        }

        let (maximized_windows, mut tiled_windows): (Vec<&FhtWindow>, Vec<&FhtWindow>) =
            self.windows.iter().partition(|w| w.is_maximized());
        tiled_windows.retain(|w| w.is_tiled());
        let tiled_windows_len = tiled_windows.len();

        let inner_gaps = CONFIG.general.inner_gaps;
        let outer_gaps = CONFIG.general.outer_gaps;

        let output_geo = self.output.geometry();
        if let Some(window) = self.fullscreen.as_ref().map(|f| &f.inner) {
            window.set_geometry(output_geo);
            if let Some(toplevel) = window.0.toplevel() {
                toplevel.send_pending_configure();
            }
        }

        let usable_geo = layer_map_for_output(&self.output)
            .non_exclusive_zone()
            .as_local()
            .to_global(&self.output);
        let mut maximized_geo = usable_geo;
        maximized_geo.size -= (2 * outer_gaps, 2 * outer_gaps).into();
        maximized_geo.loc += (outer_gaps, outer_gaps).into();
        for window in maximized_windows {
            window.set_geometry(maximized_geo);
            if let Some(toplevel) = window.0.toplevel() {
                toplevel.send_pending_configure();
            }
        }

        if !tiled_windows.is_empty() {
            let windows_len = tiled_windows.len();
            self.layouts[self.active_layout_idx].tile_windows(
                tiled_windows.into_iter(),
                windows_len,
                maximized_geo,
                inner_gaps,
            );
        }
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
        | WorkspaceLayout::BottomStack { nmaster, .. } = active_layout
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
        windows.sort_by_key(|w| std::cmp::Reverse(w.get_z_index()));

        windows
            .iter()
            .filter(|w| w.global_bbox().to_f64().contains(point))
            .find_map(|w| {
                let render_location = w.render_location();
                if w.is_in_input_region(&(point - render_location.to_f64()).as_logical()) {
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

        let old_z_index = window.get_z_index();
        let max_z_index = self.windows.iter().map(FhtWindow::get_z_index).sum::<u32>();
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
        <R as Renderer>::TextureId: 'static,

        FhtWindowRenderElement<R>: RenderElement<R>,
        WaylandSurfaceRenderElement<R>: RenderElement<R>,
    {
        if let Some(FullscreenSurface { inner, .. }) = self.fullscreen.as_ref() {
            return inner.render_elements(renderer, scale, alpha, false, true);
        }

        let mut windows = self
            .windows
            .iter()
            .enumerate()
            .map(|(idx, window)| (idx == self.focused_window_idx, window))
            .collect::<Vec<_>>();
        windows.sort_unstable_by(|a, b| a.1.get_z_index().cmp(&b.1.get_z_index()));
        windows.reverse();

        windows
            .into_iter()
            .flat_map(|(is_focused, w)| {
                w.render_elements(renderer, scale, alpha, is_focused, false)
            })
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
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// Floating layout, basically do nothing to arrange the windows.
    Floating,
}

impl WorkspaceLayout {
    /// Tile `windows` inside `tile_area` while letting `inner_gaps` between them.
    pub fn tile_windows<'a>(
        &'a self,
        windows: impl Iterator<Item = &'a FhtWindow>,
        windows_len: usize,
        tile_area: Rectangle<i32, Global>,
        inner_gaps: i32,
    ) {
        match *self {
            WorkspaceLayout::Tile {
                nmaster,
                master_width_factor,
            } => {
                // A lone master window in a workspace will basically appear the same as a
                // maximized window, so it's logical to start from there
                let mut master_geo = tile_area;
                master_geo.size.h -=
                    inner_gaps * (min(windows_len, nmaster).saturating_sub(1)) as i32;

                let mut stack_geo = tile_area;
                stack_geo.size.h -= inner_gaps * windows_len.saturating_sub(nmaster + 1) as i32;

                if windows_len > nmaster {
                    master_geo.size.w = ((master_geo.size.w - inner_gaps) as f32
                        * master_width_factor)
                        .round() as i32;
                    stack_geo.size.w -= master_geo.size.w + inner_gaps;
                    stack_geo.loc.x += master_geo.size.w + inner_gaps;
                };

                let LayoutFacts {
                    master_factor,
                    stack_factor,
                    master_rest,
                    stack_rest,
                } = getfacts(windows_len, nmaster, master_geo.size.h, stack_geo.size.h);

                for (idx, window) in windows.enumerate() {
                    if idx < nmaster {
                        let mut master_height =
                            (master_geo.size.h as f32 / master_factor).round() as i32;
                        master_height += ((idx as f32) < master_rest) as i32;

                        window.set_geometry(Rectangle::from_loc_and_size(
                            master_geo.loc,
                            (master_geo.size.w, master_height),
                        ));

                        master_geo.loc.y += master_height + inner_gaps;
                    } else {
                        let mut stack_height =
                            (stack_geo.size.h as f32 / stack_factor).round() as i32;
                        stack_height += ((idx as f32) < stack_rest) as i32;

                        window.set_geometry(Rectangle::from_loc_and_size(
                            stack_geo.loc,
                            (stack_geo.size.w, stack_height),
                        ));

                        stack_geo.loc.y += stack_height + inner_gaps;
                    }

                    if let Some(toplevel) = window.0.toplevel() {
                        toplevel.send_pending_configure();
                    }
                }
            }
            WorkspaceLayout::BottomStack {
                nmaster,
                master_width_factor,
            } => {
                // A lone master window in a workspace will basically appear the same as a
                // maximized window, so it's logical to start from there
                let mut master_geo = tile_area;
                master_geo.size.w -=
                    inner_gaps * (min(windows_len, nmaster).saturating_sub(1)) as i32;

                let mut stack_geo = tile_area;
                stack_geo.size.w -= inner_gaps * windows_len.saturating_sub(nmaster + 1) as i32;

                if windows_len > nmaster {
                    stack_geo.size.h = ((stack_geo.size.h - inner_gaps) as f32
                        * (1f32 - master_width_factor))
                        .round() as i32;
                    master_geo.size.h -= stack_geo.size.h + inner_gaps;
                    stack_geo.loc.y += master_geo.size.h + inner_gaps;
                };

                let LayoutFacts {
                    master_factor,
                    stack_factor,
                    master_rest,
                    stack_rest,
                } = getfacts(windows_len, nmaster, master_geo.size.w, stack_geo.size.w);

                for (idx, window) in windows.enumerate() {
                    if idx < nmaster {
                        let mut master_width =
                            (master_geo.size.w as f32 / master_factor).round() as i32;
                        master_width += ((idx as f32) < master_rest) as i32;

                        window.set_geometry(Rectangle::from_loc_and_size(
                            master_geo.loc,
                            (master_width, master_geo.size.h),
                        ));

                        master_geo.loc.x += master_width + inner_gaps;
                    } else {
                        let mut stack_width =
                            (stack_geo.size.w as f32 / stack_factor).round() as i32;
                        stack_width += ((idx as f32) < stack_rest) as i32;

                        window.set_geometry(Rectangle::from_loc_and_size(
                            stack_geo.loc,
                            (stack_width, stack_geo.size.h),
                        ));

                        stack_geo.loc.x += stack_width + inner_gaps;
                    }

                    if let Some(toplevel) = window.0.toplevel() {
                        toplevel.send_pending_configure();
                    }
                }
            }
            WorkspaceLayout::Floating => {
                // Let the windows be free
                for window in windows {
                    if let Some(toplevel) = window.0.toplevel() {
                        toplevel.send_pending_configure();
                    }
                }
            }
        }
    }
}

pub struct LayoutFacts {
    /// Total factor of the master area
    pub master_factor: f32,
    /// Total factor of the stack area
    pub stack_factor: f32,
    /// Remainder of the master area after an even split
    pub master_rest: f32,
    /// Remainder of the stack area after an even split
    pub stack_rest: f32,
}

pub fn getfacts(
    windows_len: usize,
    nmaster: usize,
    master_size: i32,
    stack_size: i32,
) -> LayoutFacts {
    let master_factor = min(windows_len, nmaster) as f32;
    let stack_factor = (windows_len - nmaster) as f32;
    let mut master_rest @ mut stack_rest = master_size as f32;

    for i in 0..windows_len {
        if i < nmaster {
            master_rest -= master_size as f32 / master_factor as f32;
        } else {
            stack_rest -= stack_size as f32 / stack_factor as f32;
        }
    }

    LayoutFacts {
        master_factor,
        stack_factor,
        master_rest,
        stack_rest,
    }
}
