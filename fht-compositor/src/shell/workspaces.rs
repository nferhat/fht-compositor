use std::cmp::min;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::utils::{
    CropRenderElement, Relocate, RelocateRenderElement,
};
use smithay::backend::renderer::element::{Element, RenderElement};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::desktop::layer_map_for_output;
use smithay::desktop::space::SpaceElement;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale};
use smithay::wayland::seat::WaylandFocus;
use tween::{ExpoOut, Tween, Tweener};

use super::window::FhtWindowRenderElement;
use super::FhtWindow;
use crate::backend::render::AsGlowRenderer;
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};
use crate::config::CONFIG;
use crate::utils::geometry::{PointGlobalExt, RectExt, RectGlobalExt, RectLocalExt, SizeExt};
use crate::utils::output::OutputExt;

pub struct WorkspaceSet {
    pub(super) output: Output,
    pub workspaces: Vec<Workspace>,
    pub switch_animation: Option<WorkspaceSwitchAnimation>,
    active_idx: AtomicUsize,
}

impl WorkspaceSet {
    pub fn new(output: Output) -> Self {
        Self {
            output: output.clone(),
            workspaces: (0..9).map(|_| Workspace::new(output.clone())).collect(),
            switch_animation: None,
            active_idx: 0.into(),
        }
    }

    pub fn refresh(&mut self) {
        let _ = self
            .switch_animation
            .take_if(|anim| anim.tweener.is_finished());
        if let Some(animation) = self.switch_animation.as_mut() {
            animation.advance();
            if animation.tweener.is_finished() {
                self.active_idx
                    .store(animation.target_idx, Ordering::SeqCst);
            }
        }

        self.workspaces_mut().for_each(Workspace::refresh);
    }

    pub fn set_active_idx(&mut self, target_idx: usize) -> Option<FhtWindow> {
        let target_idx = target_idx.clamp(0, 9);
        let active_idx = self.active_idx.load(Ordering::SeqCst);
        if target_idx == active_idx || self.switch_animation.is_some() {
            return None;
        }

        self.switch_animation = Some(WorkspaceSwitchAnimation::new(
            target_idx,
            if target_idx > active_idx {
                WorkspaceSwitchDirection::Next
            } else {
                WorkspaceSwitchDirection::Previous
            },
        ));

        self.workspaces[target_idx].focused().cloned()
    }

    pub fn workspaces(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces.iter()
    }

    pub fn workspaces_mut(&mut self) -> impl Iterator<Item = &mut Workspace> {
        self.workspaces.iter_mut()
    }

    pub fn active(&self) -> &Workspace {
        &self.workspaces[self.active_idx.load(Ordering::SeqCst)]
    }

    pub fn active_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active_idx.load(Ordering::SeqCst)]
    }

    pub fn all_windows(&mut self) -> impl Iterator<Item = &FhtWindow> {
        self.workspaces().flat_map(|ws| &ws.windows)
    }

    pub fn arrange(&self) {
        self.workspaces()
            .for_each(Workspace::refresh_window_geometries)
    }

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

    pub fn find_workspace(&self, surface: &WlSurface) -> Option<&Workspace> {
        self.workspaces().find(|ws| {
            ws.windows
                .iter()
                .any(|w| w.wl_surface().as_ref() == Some(surface))
        })
    }

    pub fn find_workspace_mut(&mut self, surface: &WlSurface) -> Option<&mut Workspace> {
        self.workspaces_mut().find(|ws| {
            ws.windows
                .iter()
                .any(|w| w.wl_surface().as_ref() == Some(surface))
        })
    }

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

    pub fn ws_for(&self, window: &FhtWindow) -> Option<&Workspace> {
        self.workspaces()
            .find(|ws| ws.windows.iter().any(|w| w == window))
    }

    pub fn ws_mut_for(&mut self, window: &FhtWindow) -> Option<&mut Workspace> {
        self.workspaces_mut()
            .find(|ws| ws.windows.iter().any(|w| w == window))
    }

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
        let active = self.active();
        let output_geo: Rectangle<i32, Physical> = self
            .output
            .geometry()
            .as_logical()
            .to_physical_precise_round(scale);

        if self.switch_animation.is_none() {
            let active_elements = active.render_elements(renderer, scale, alpha);
            elements.extend(
                active_elements
                    .into_iter()
                    .map(WorkspaceSetRenderElement::Normal),
            );

            return (active.fullscreen.is_some(), elements);
        }

        let Some(animation) = self.switch_animation.as_ref() else {
            unreachable!()
        };
        if animation.tweener.is_finished() {
            self.active_idx
                .store(animation.target_idx, Ordering::SeqCst);
        }
        let active_elements = active.render_elements(renderer, scale, alpha);
        let target = &self.workspaces[animation.target_idx];
        let target_elements = target.render_elements(renderer, scale, alpha);

        let (current_offset, target_offset) = match animation.direction {
            WorkspaceSwitchDirection::Next => {
                // Focusing the next offset.
                // For the active, how much should we *remove* from the current position
                // For the target, how much should we add to the current position
                let offset = (animation.current_offset * output_geo.size.w as f64).round() as i32;
                (offset - output_geo.size.w, offset)
            }
            WorkspaceSwitchDirection::Previous => {
                // Focusing a previous workspace
                // For the active, how much should we add to tyhe current position
                // For the target, how much should we remove from the current position.
                let offset = (animation.current_offset * output_geo.size.w as f64).round() as i32;
                (-offset + output_geo.size.w, -offset)
            }
        };

        elements.extend(active_elements.into_iter().filter_map(|element| {
            let offset = Point::from((current_offset, 0));
            let relocate = RelocateRenderElement::from_element(element, offset, Relocate::Relative);
            let crop = CropRenderElement::from_element(relocate, scale, output_geo)?;
            Some(WorkspaceSetRenderElement::Switching(crop))
        }));
        elements.extend(target_elements.into_iter().filter_map(|element| {
            let offset = Point::from((target_offset, 0));
            let relocate = RelocateRenderElement::from_element(element, offset, Relocate::Relative);
            let crop = CropRenderElement::from_element(relocate, scale, output_geo)?;
            Some(WorkspaceSetRenderElement::Switching(crop))
        }));

        (
            active.fullscreen.is_some() || target.fullscreen.is_some(),
            elements,
        )
    }
}

pub struct WorkspaceSwitchAnimation {
    // pub switch_animation: Option<(Tweener<f64, f64, Box<dyn Tween<f64>>>, Instant, usize, f64)>,
    tweener: Tweener<f64, f64, Box<dyn Tween<f64>>>,
    direction: WorkspaceSwitchDirection,
    current_offset: f64,
    started_at: Instant,
    pub target_idx: usize,
}

impl WorkspaceSwitchAnimation {
    fn new(target_idx: usize, direction: WorkspaceSwitchDirection) -> Self {
        let tween = Box::new(|delta, percent| ExpoOut.tween(delta, percent)) as Box<dyn Tween<f64>>;

        // When going to the next workspace, the values describes the offset of the next workspace.
        // When going to the previous workspace, the values describe the offset of the current
        // workspace

        let tweener = Tweener::new(1.0, 0.0, 400.0, tween);

        Self {
            tweener,
            direction,
            current_offset: 0.0,
            started_at: Instant::now(),
            target_idx,
        }
    }

    /// Advance the animation.
    #[profiling::function]
    fn advance(&mut self) {
        // Advance the tween by however much we need
        // NOTE: The duration is in MILLISECONDS, and we should also prob add a blocker for
        // animated clients.

        self.current_offset = self
            .tweener
            .move_to(self.started_at.elapsed().as_millis() as f64);
    }
}

enum WorkspaceSwitchDirection {
    Next,
    Previous,
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
    Switching(CropRenderElement<RelocateRenderElement<FhtWindowRenderElement<R>>>),
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
    ) -> Vec<Rectangle<i32, smithay::utils::Physical>> {
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

#[derive(Debug)]
pub struct Workspace {
    output: Output,

    pub windows: Vec<FhtWindow>,
    pub fullscreen: Option<FullscreenSurface>,
    pub focused_window_idx: usize,

    pub layouts: Vec<WorkspaceLayout>,
    pub active_layout_idx: usize,
}

impl Workspace {
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
        for (idx, window) in self.windows.iter().enumerate() {
            window.set_activate(idx == self.focused_window_idx);
            window.refresh();
        }
    }

    pub fn focused(&self) -> Option<&FhtWindow> {
        self.windows
            .get(self.focused_window_idx)
            .or_else(|| self.fullscreen.as_ref().map(|f| &f.inner))
    }

    pub fn insert_window(&mut self, window: FhtWindow) {
        if self.windows.contains(&window) {
            return;
        }

        // Configure the window for insertion
        window.output_enter(&self.output, window.bbox());
        window.set_bounds(Some(self.output.geometry().size.as_logical()));

        self.windows.push(window);
        if CONFIG.general.focus_new_windows {
            self.focused_window_idx = self.windows.len() - 1;
        }
        self.refresh_window_geometries();
    }

    pub fn remove_window(&mut self, window: &FhtWindow) -> Option<FhtWindow> {
        let Some(idx) = self.windows.iter().position(|w| w == window) else {
            return None;
        };

        let window = self.windows.remove(idx);
        // "Un"-configure the window (for potentially inserting it on another workspace who knows)
        window.output_leave(&self.output);
        window.set_bounds(None);

        self.refresh_window_geometries();
        Some(window)
    }

    pub fn focus_window(&mut self, window: &FhtWindow) {
        if let Some(idx) = self.windows.iter().position(|w| w == window) {
            self.focused_window_idx = idx;
            self.refresh();
        }
    }

    pub fn focus_next_window(&mut self) -> Option<&FhtWindow> {
        if self.windows.is_empty() {
            return None;
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

    pub fn focus_previous_window(&mut self) -> Option<&FhtWindow> {
        if self.windows.is_empty() {
            return None;
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

    pub fn fullscreen_window(&mut self, window: &FhtWindow) {
        let Some(idx) = self.windows.iter().position(|w| w == window) else {
            return;
        };

        let window = self.windows.remove(idx);
        self.fullscreen = Some(FullscreenSurface {
            inner: window,
            last_known_idx: idx,
        });
        self.refresh_window_geometries();
    }

    #[profiling::function]
    pub fn refresh_window_geometries(&self) {
        let active_layout = self.layouts[self.active_layout_idx];
        if self.windows.is_empty() || active_layout == WorkspaceLayout::Floating {
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
        }

        let output_geo = layer_map_for_output(&self.output)
            .non_exclusive_zone()
            .as_local();
        let mut maximized_geo = output_geo;
        maximized_geo.size -= (2 * outer_gaps, 2 * outer_gaps).into();
        maximized_geo.loc += (outer_gaps, outer_gaps).into();
        for window in maximized_windows {
            window.set_geometry(maximized_geo.to_global(&self.output));
            if let Some(toplevel) = window.0.toplevel() {
                toplevel.send_pending_configure();
            }
        }

        if tiled_windows.is_empty() {
            return;
        }

        match active_layout {
            WorkspaceLayout::Tile {
                nmaster,
                master_width_factor,
            } => {
                // A lone master window in a workspace will basically appear the same as a
                // maximized window, so it's logical to start from there
                let mut master_geo = maximized_geo;
                master_geo.size.h -=
                    inner_gaps * (min(tiled_windows_len, nmaster).saturating_sub(1)) as i32;

                let mut stack_geo = maximized_geo;
                stack_geo.size.h -=
                    inner_gaps * tiled_windows_len.saturating_sub(nmaster + 1) as i32;

                if tiled_windows_len > nmaster {
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
                } = getfacts(
                    tiled_windows_len,
                    nmaster,
                    master_geo.size.h,
                    stack_geo.size.h,
                );

                for (idx, window) in tiled_windows.iter().enumerate() {
                    if idx < nmaster {
                        let mut master_height =
                            (master_geo.size.h as f32 / master_factor).round() as i32;
                        master_height += ((idx as f32) < master_rest) as i32;

                        window.set_geometry(
                            Rectangle::from_loc_and_size(
                                master_geo.loc,
                                (master_geo.size.w, master_height),
                            )
                            .to_global(&self.output),
                        );

                        master_geo.loc.y += master_height + inner_gaps;
                    } else {
                        let mut stack_height =
                            (stack_geo.size.h as f32 / stack_factor).round() as i32;
                        stack_height += ((idx as f32) < stack_rest) as i32;

                        window.set_geometry(
                            Rectangle::from_loc_and_size(
                                stack_geo.loc,
                                (stack_geo.size.w, stack_height),
                            )
                            .to_global(&self.output),
                        );

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
                let mut master_geo = maximized_geo;
                master_geo.size.w -=
                    inner_gaps * (min(tiled_windows_len, nmaster).saturating_sub(1)) as i32;

                let mut stack_geo = maximized_geo;
                stack_geo.size.w -=
                    inner_gaps * tiled_windows_len.saturating_sub(nmaster + 1) as i32;

                if tiled_windows_len > nmaster {
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
                } = getfacts(
                    tiled_windows_len,
                    nmaster,
                    master_geo.size.w,
                    stack_geo.size.w,
                );

                for (idx, window) in tiled_windows.iter().enumerate() {
                    if idx < nmaster {
                        let mut master_width =
                            (master_geo.size.w as f32 / master_factor).round() as i32;
                        master_width += ((idx as f32) < master_rest) as i32;

                        window.set_geometry(
                            Rectangle::from_loc_and_size(
                                master_geo.loc,
                                (master_width, master_geo.size.h),
                            )
                            .to_global(&self.output),
                        );

                        master_geo.loc.x += master_width + inner_gaps;
                    } else {
                        let mut stack_width =
                            (stack_geo.size.w as f32 / stack_factor).round() as i32;
                        stack_width += ((idx as f32) < stack_rest) as i32;

                        window.set_geometry(
                            Rectangle::from_loc_and_size(
                                stack_geo.loc,
                                (stack_width, stack_geo.size.h),
                            )
                            .to_global(&self.output),
                        );

                        stack_geo.loc.x += stack_width + inner_gaps;
                    }

                    if let Some(toplevel) = window.0.toplevel() {
                        toplevel.send_pending_configure();
                    }
                }
            }
            WorkspaceLayout::Floating => {
                // Let the windows be free
            }
        }
    }

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

    pub fn select_previous_layout(&mut self) {
        let layouts_len = self.layouts.len();
        let new_active_idx = match self.active_layout_idx.checked_sub(1) {
            Some(idx) => idx,
            None => layouts_len - 1,
        };

        self.active_layout_idx = new_active_idx;
        self.refresh_window_geometries();
    }

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

    pub fn change_nmaster(&mut self, delta: i32) {
        let active_layout = &mut self.layouts[self.active_layout_idx];
        if let WorkspaceLayout::Tile { nmaster, .. }
        | WorkspaceLayout::BottomStack { nmaster, .. } = active_layout
        {
            *nmaster = nmaster.saturating_add_signed(delta as isize);
        }
        self.refresh_window_geometries();
    }

    #[profiling::function]
    pub fn window_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(&FhtWindow, Point<i32, Logical>)> {
        if let Some(FullscreenSurface { inner, .. }) = self.fullscreen.as_ref() {
            return Some((inner, inner.render_location().as_logical()));
        }

        let mut windows = self.windows.iter().collect::<Vec<_>>();
        windows.sort_by_key(|w| std::cmp::Reverse(w.get_z_index()));

        windows
            .iter()
            .filter(|w| w.global_bbox().to_f64().as_logical().contains(point))
            .find_map(|w| {
                let render_location = w.render_location().as_logical();
                if w.is_in_input_region(&(point - render_location.to_f64())) {
                    Some((*w, render_location))
                } else {
                    None
                }
            })
    }

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
        let mut elements = vec![];

        if let Some(FullscreenSurface { inner, .. }) = self.fullscreen.as_ref() {
            elements.extend(inner.render_elements(
                renderer,
                scale,
                alpha,
                &self.output,
                false,
                true,
            ));
        }

        let mut windows = self
            .windows
            .iter()
            .enumerate()
            .map(|(idx, window)| (idx == self.focused_window_idx, window))
            .collect::<Vec<_>>();
        windows.sort_unstable_by(|a, b| a.1.get_z_index().cmp(&b.1.get_z_index()));
        windows.reverse();

        elements.extend(windows.into_iter().flat_map(|(is_focused, window)| {
            window.render_elements(renderer, scale, alpha, &self.output, is_focused, false)
        }));

        elements
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WorkspaceLayout {
    Tile {
        nmaster: usize,
        master_width_factor: f32,
    },
    BottomStack {
        nmaster: usize,
        master_width_factor: f32,
    },
    Floating,
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
