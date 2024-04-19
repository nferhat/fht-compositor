mod surface;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{Element, Id, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::element::PixelShaderElement;
use smithay::backend::renderer::gles::GlesError;
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::desktop::space::{RenderZindex, SpaceElement};
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::desktop::{PopupManager, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as XdgToplevelState;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, IsAlive, Logical, Physical, Point, Rectangle, Scale, Size};
use smithay::wayland::compositor::{
    with_states, with_surface_tree_downward, SurfaceData as WlSurfaceData, TraversalAction,
};
use smithay::wayland::dmabuf::DmabufFeedback;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgToplevelSurfaceData};
pub use surface::*;

use super::decorations::RoundedOutlineShader;
#[cfg(feature = "udev_backend")]
use crate::backend::render::AsGlowFrame;
use crate::backend::render::AsGlowRenderer;
#[cfg(feature = "udev_backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};
use crate::config::{BorderConfig, CONFIG};
use crate::utils::geometry::{Global, PointExt, PointGlobalExt, RectExt, RectGlobalExt, SizeExt};

pub struct FhtWindowData {
    pub border_config: Option<BorderConfig>,
    pub location: Point<i32, Global>,
    pub z_index: u32,
    pub last_floating_geometry: Option<Rectangle<i32, Global>>,
}

#[derive(Clone)]
pub struct FhtWindow {
    pub(crate) surface: FhtWindowSurface,
    data: Arc<Mutex<FhtWindowData>>,
}

impl PartialEq for FhtWindow {
    fn eq(&self, other: &Self) -> bool {
        self.surface == other.surface && Arc::ptr_eq(&self.data, &other.data)
    }
}

impl std::fmt::Debug for FhtWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FhtWindow")
            .field("surface", &self.wl_surface().id().protocol_id())
            .field("data", &"...")
            .finish()
    }
}

// Even if we dont use all the getters/setters its not that bad since they get removed in code
// analysis when compiling, and I find the warnings annoying soo.
#[allow(dead_code)]
impl FhtWindow {
    pub fn new(surface: FhtWindowSurface, border_config: Option<BorderConfig>) -> Self {
        Self {
            surface,
            data: Arc::new(Mutex::new(FhtWindowData {
                border_config,
                location: Point::default(),
                z_index: RenderZindex::Shell as u32,
                last_floating_geometry: None,
            })),
        }
    }

    pub fn uid(&self) -> u64 {
        self.surface.wl_surface().unwrap().id().protocol_id() as u64
    }

    pub fn toplevel(&self) -> &ToplevelSurface {
        self.surface.toplevel()
    }

    pub fn location(&self) -> Point<i32, Global> {
        self.data.lock().unwrap().location
    }

    pub fn render_location(&self) -> Point<i32, Global> {
        self.location() - self.surface.geometry().loc.as_global()
    }

    pub fn border_config(&self) -> BorderConfig {
        self.data.lock().unwrap().border_config.unwrap_or(CONFIG.decoration.border)
    }

    pub fn geometry(&self) -> Rectangle<i32, Global> {
        let mut geo = self.surface.geometry().as_global();
        geo.loc = self.location();
        geo
    }

    pub fn set_geometry(&self, geometry: Rectangle<i32, Global>) {
        self.surface.toplevel().with_pending_state(|state| {
            state.size = Some(geometry.size.as_logical());
        });

        let mut data = self.data.lock().unwrap();
        data.location = geometry.loc;

        if !self.tiled() {
            data.last_floating_geometry = Some(geometry)
        }
    }

    pub fn set_geometry_with_border(&self, mut geometry: Rectangle<i32, Global>) {
        let thickness = self.border_config().thickness as i32;
        geometry.loc.x += thickness as i32;
        geometry.loc.y += thickness as i32;
        geometry.size.w -= 2 * thickness as i32;
        geometry.size.h -= 2 * thickness as i32;
        self.set_geometry(geometry);
    }

    pub fn bbox(&self) -> Rectangle<i32, Global> {
        let mut bbox = self.surface.bbox().as_global();
        bbox.loc = self.render_location();
        bbox
    }

    pub fn fullscreen(&self) -> bool {
        self.surface
            .toplevel()
            .with_pending_state(|state| state.states.contains(XdgToplevelState::Fullscreen))
    }

    pub fn set_fullscreen(&self, fullscreen: bool, fullscreen_output: Option<WlOutput>) {
        self.surface.toplevel().with_pending_state(|state| {
            if fullscreen {
                state.states.set(XdgToplevelState::Fullscreen)
            } else {
                state.states.unset(XdgToplevelState::Fullscreen)
            };
            state.fullscreen_output = fullscreen_output;
        });
    }

    pub fn maximized(&self) -> bool {
        self.surface
            .toplevel()
            .with_pending_state(|state| state.states.contains(XdgToplevelState::Maximized))
    }

    pub fn set_maximized(&self, maximized: bool) {
        self.surface.toplevel().with_pending_state(|state| {
            if maximized {
                state.states.set(XdgToplevelState::Maximized)
            } else {
                state.states.unset(XdgToplevelState::Maximized)
            }
        });
    }

    pub fn tiled(&self) -> bool {
        self.surface.toplevel().with_pending_state(|state| {
            state.states.contains(XdgToplevelState::TiledLeft)
                || state.states.contains(XdgToplevelState::TiledRight)
                || state.states.contains(XdgToplevelState::TiledTop)
                || state.states.contains(XdgToplevelState::TiledBottom)
        })
    }

    pub fn set_tiled(&self, tiled: bool) {
        self.surface.toplevel().with_pending_state(|state| {
            if tiled {
                state.states.set(XdgToplevelState::TiledLeft);
                state.states.set(XdgToplevelState::TiledRight);
                state.states.set(XdgToplevelState::TiledTop);
                state.states.set(XdgToplevelState::TiledBottom);
            } else {
                state.states.unset(XdgToplevelState::TiledLeft);
                state.states.unset(XdgToplevelState::TiledRight);
                state.states.unset(XdgToplevelState::TiledTop);
                state.states.unset(XdgToplevelState::TiledBottom);
            };
        });
    }

    pub fn bounds(&self) -> Option<Size<i32, Logical>> {
        self.surface
            .toplevel()
            .with_pending_state(|state| state.bounds)
    }

    pub fn set_bounds(&self, bounds: Option<Size<i32, Logical>>) {
        self.surface.toplevel().with_pending_state(|state| {
            state.bounds = bounds;
        });
    }

    pub fn activated(&self) -> bool {
        self.surface
            .toplevel()
            .with_pending_state(|state| state.states.contains(XdgToplevelState::Activated))
    }

    pub fn set_activated(&self, activated: bool) {
        self.surface.toplevel().with_pending_state(|state| {
            if activated {
                state.states.set(XdgToplevelState::Activated)
            } else {
                state.states.unset(XdgToplevelState::Activated)
            }
        });
    }

    pub fn z_index(&self) -> u32 {
        self.data.lock().unwrap().z_index
    }

    pub fn set_z_index(&self, z_index: u32) {
        self.data.lock().unwrap().z_index = z_index
    }

    pub fn app_id(&self) -> String {
        with_states(&self.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .app_id
                .clone()
                .unwrap_or_default()
        })
    }

    pub fn title(&self) -> String {
        with_states(&self.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .title
                .clone()
                .unwrap_or_default()
        })
    }

    pub fn wl_surface(&self) -> WlSurface {
        // SAFETY: FhtWindow is mapped, so a WlSurface is always available.
        self.surface.wl_surface().unwrap()
    }

    /// Return whether this window owns this [`WlSurface`] with surface type [`WindowSurfaceType`]
    ///
    /// You can with this check if a window owns a popup, for example.
    pub fn has_surface(&self, surface: &WlSurface, surface_type: WindowSurfaceType) -> bool {
        let self_surface = self.wl_surface();

        if surface_type.contains(WindowSurfaceType::TOPLEVEL) && self_surface == *surface {
            return true;
        }

        if surface_type.contains(WindowSurfaceType::SUBSURFACE) {
            use std::sync::atomic::Ordering;
            let found_surface = AtomicBool::new(false);
            with_surface_tree_downward(
                &self_surface,
                surface,
                |_, _, s| TraversalAction::DoChildren(s),
                |s, _, search| {
                    found_surface.fetch_or(s == *search, Ordering::SeqCst);
                },
                |_, _, _| !found_surface.load(Ordering::SeqCst),
            );
            if found_surface.load(Ordering::SeqCst) {
                return true;
            }
        }

        if surface_type.contains(WindowSurfaceType::POPUP) {
            return PopupManager::popups_for_surface(&self_surface)
                .any(|(p, _)| p.wl_surface() == surface);
        }

        false
    }

    /// Return the topmost surface owned by this window under this point.
    ///
    /// NOTE: This function expects `point` to be relative to the window origin. You can achieve
    /// this by offseting it by [`Self::render_location`]
    pub fn surface_under(
        &self,
        point: Point<f64, Logical>,
        surface_type: WindowSurfaceType,
    ) -> Option<(WlSurface, Point<i32, Logical>)> {
        self.surface.inner.surface_under(point, surface_type)
    }

    /// Run a closure on all the window surfaces.
    pub fn with_surfaces<F>(&self, processor: F)
    where
        F: FnMut(&WlSurface, &WlSurfaceData),
    {
        self.surface.inner.with_surfaces(processor)
    }

    /// Send frame callbacks to all surfaces of this window.
    pub fn send_frame<T, F>(
        &self,
        output: &Output,
        time: T,
        throttle: Option<Duration>,
        primary_scan_out_output: F,
    ) where
        T: Into<Duration>,
        F: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
    {
        self.surface
            .inner
            .send_frame(output, time, throttle, primary_scan_out_output)
    }

    /// Send dmabuf feedback to all surfaces of this window.
    pub fn send_dmabuf_feedback<'a, P, F>(
        &self,
        output: &Output,
        primary_scan_out_output: P,
        select_dmabuf_feedback: F,
    ) where
        P: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
        F: Fn(&WlSurface, &WlSurfaceData) -> &'a DmabufFeedback + Copy,
    {
        self.surface.inner.send_dmabuf_feedback(
            output,
            primary_scan_out_output,
            select_dmabuf_feedback,
        )
    }

    pub fn take_presentation_feedback<F1, F2>(
        &self,
        output_feedback: &mut OutputPresentationFeedback,
        primary_scan_out_output: F1,
        presentation_feedback_flags: F2,
    ) where
        F1: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
        F2: FnMut(&WlSurface, &WlSurfaceData) -> wp_presentation_feedback::Kind + Copy,
    {
        self.surface.inner.take_presentation_feedback(
            output_feedback,
            primary_scan_out_output,
            presentation_feedback_flags,
        )
    }

    /// Close this window.
    pub fn close(&self) {
        if let Some(toplevel) = self.surface.inner.toplevel() {
            toplevel.send_close();
            return;
        }
    }

    /// Get access to this window [`UserDataMap`]
    pub fn user_data(&self) -> &UserDataMap {
        self.surface.inner.user_data()
    }
}

impl IsAlive for FhtWindow {
    fn alive(&self) -> bool {
        self.surface.alive()
    }
}

#[derive(Debug)]
pub enum FhtWindowRenderElement<R>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
{
    Normal(FhtWindowSurfaceRenderElement<R>),
    Border(PixelShaderElement),
}

impl<R> Element for FhtWindowRenderElement<R>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
{
    fn id(&self) -> &Id {
        match self {
            Self::Normal(e) => e.id(),
            Self::Border(e) => e.id(),
        }
    }

    fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
        match self {
            Self::Normal(e) => e.current_commit(),
            Self::Border(e) => e.current_commit(),
        }
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        match self {
            Self::Normal(e) => e.src(),
            Self::Border(e) => e.src(),
        }
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        match self {
            Self::Normal(e) => e.geometry(scale),
            Self::Border(e) => e.geometry(scale),
        }
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        match self {
            Self::Normal(e) => e.location(scale),
            Self::Border(e) => e.location(scale),
        }
    }

    fn transform(&self) -> smithay::utils::Transform {
        match self {
            Self::Normal(e) => e.transform(),
            Self::Border(e) => e.transform(),
        }
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<smithay::backend::renderer::utils::CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
        match self {
            Self::Normal(e) => e.damage_since(scale, commit),
            Self::Border(e) => e.damage_since(scale, commit),
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        match self {
            Self::Normal(e) => e.opaque_regions(scale),
            Self::Border(e) => e.opaque_regions(scale),
        }
    }

    fn alpha(&self) -> f32 {
        match self {
            Self::Normal(e) => e.alpha(),
            Self::Border(e) => e.alpha(),
        }
    }

    fn kind(&self) -> smithay::backend::renderer::element::Kind {
        match self {
            Self::Normal(e) => e.kind(),
            Self::Border(e) => e.kind(),
        }
    }
}

impl RenderElement<GlowRenderer> for FhtWindowRenderElement<GlowRenderer> {
    fn draw(
        &self,
        frame: &mut GlowFrame,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        match self {
            Self::Normal(e) => e.draw(frame, src, dst, damage),
            Self::Border(e) => <PixelShaderElement as RenderElement<GlowRenderer>>::draw(
                e, frame, src, dst, damage,
            ),
        }
    }

    fn underlying_storage(&self, renderer: &mut GlowRenderer) -> Option<UnderlyingStorage> {
        match self {
            Self::Normal(e) => e.underlying_storage(renderer),
            Self::Border(e) => e.underlying_storage(renderer),
        }
    }
}

#[cfg(feature = "udev_backend")]
impl<'a> RenderElement<UdevRenderer<'a>> for FhtWindowRenderElement<UdevRenderer<'a>> {
    fn draw(
        &self,
        frame: &mut UdevFrame<'a, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), UdevRenderError> {
        match self {
            Self::Normal(e) => e.draw(frame, src, dst, damage),
            Self::Border(e) => {
                let frame = frame.glow_frame_mut();
                <PixelShaderElement as RenderElement<GlowRenderer>>::draw(
                    e, frame, src, dst, damage,
                )
                .map_err(|err| UdevRenderError::Render(err))
            }
        }
    }

    fn underlying_storage(&self, renderer: &mut UdevRenderer<'a>) -> Option<UnderlyingStorage> {
        match self {
            Self::Normal(e) => e.underlying_storage(renderer),
            Self::Border(e) => {
                let renderer = renderer.glow_renderer_mut();
                e.underlying_storage(renderer)
            }
        }
    }
}

impl FhtWindow {
    #[profiling::function]
    pub fn render_elements<R>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<FhtWindowRenderElement<R>>
    where
        R: Renderer + ImportAll + AsGlowRenderer + ImportMem,
        <R as Renderer>::TextureId: 'static,
        WaylandSurfaceRenderElement<R>: RenderElement<R>,
    {
        let surface = self.wl_surface();
        // If the window is fullscreen then it's edge to edge with no border and rounding.
        // Otherwise, the window is not edge to edge and can be drawn normally.
        let border_config = (!self.fullscreen()).then(|| self.border_config());
        let render_location = self.render_location();
        let geometry = self.geometry();

        let mut render_elements = vec![];

        let (window_elements, popup_elements) = self.surface.render_elements(
            renderer,
            render_location
                .as_logical()
                .to_physical_precise_round(scale),
            scale,
            alpha,
            border_config.map(|bc| bc.radius),
        );
        render_elements.extend(
            popup_elements
                .into_iter()
                .map(FhtWindowRenderElement::Normal),
        );

        if let Some(border_config) = border_config {
            let border_element = RoundedOutlineShader::element(
                renderer,
                scale.x.max(scale.y), // WARN: This may not be always accurate.
                alpha,
                &surface,
                geometry.as_logical().as_local(),
                super::decorations::RoundedOutlineShaderSettings {
                    thickness: border_config.thickness,
                    radius: border_config.radius,
                    color: if self.activated() {
                        border_config.focused_color
                    } else {
                        border_config.normal_color
                    },
                },
            );
            render_elements.push(FhtWindowRenderElement::Border(border_element));
        }

        render_elements.extend(
            window_elements
                .into_iter()
                .map(FhtWindowRenderElement::Normal),
        );

        render_elements
    }
}
