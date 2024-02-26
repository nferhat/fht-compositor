//! A custom shell window.
//!
//! [`FhtWindow`] is a tagged union struct for other high level window abstractions from Smithay
//! (respectively a wayland [`Window`] or a Xwayland [`X11Surface`]), it has additional data
//! attached to it in a form of user data stored inside the [`UserDataMap`] of the underlying
//! constructs
//!
//! For rendering the [`FhtWindowRenderElement`] separates between the toplevel surface, any
//! subsurfaces (like popups), and the window border.

use std::borrow::BorrowMut;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::Arc;
use std::time::Duration;

use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::{Element, Kind, RenderElement};
use smithay::backend::renderer::gles::element::PixelShaderElement;
use smithay::backend::renderer::gles::{GlesError, GlesFrame, Uniform};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::{ImportAll, Renderer};
use smithay::desktop::space::SpaceElement;
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::desktop::{PopupManager, Window, WindowSurfaceType};
use smithay::input::keyboard::KeyboardTarget;
use smithay::input::pointer::PointerTarget;
use smithay::input::Seat;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as XdgToplevelState;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Size};
use smithay::wayland::compositor::{
    with_states, with_surface_tree_downward, SurfaceData as WlSurfaceData, TraversalAction,
};
use smithay::wayland::dmabuf::DmabufFeedback;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgToplevelSurfaceData};
#[cfg(feature = "xwayland")]
use smithay::xwayland::X11Surface;

use super::decorations::{RoundedOutlineShader, RoundedOutlineShaderSettings, RoundedQuadShader};
#[cfg(feature = "udev_backend")]
use crate::backend::render::AsGlowFrame;
use crate::backend::render::AsGlowRenderer;
#[cfg(feature = "udev_backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};
use crate::config::CONFIG;
use crate::state::State;
#[cfg(feature = "xwayland")]
use crate::utils::geometry::RectGlobalExt;
use crate::utils::geometry::{Global, PointExt, PointGlobalExt, RectExt, SizeExt};

pub struct FhtWindowData {
    /// NOTE: The window doesn't manage this in any way, it's up to the workspace it's on to manage
    /// the Z-index relative to other windows.
    pub z_index: Arc<AtomicU32>,

    pub location: Point<i32, Global>,
    pub last_floating_geometry: Option<Rectangle<i32, Global>>,
}

/// An abstraction over smithay's builtin [`Window`] type.
#[derive(Debug, Clone, PartialEq, Hash)]
pub struct FhtWindow(pub Window);

/// Since [`X11Surface`]s don't have a way to store whether they are tiled or not unlike wayland
/// windows that do through the xdg-shell protocol
#[cfg(feature = "xwayland")]
pub struct X11SurfaceTiled(AtomicBool);

impl FhtWindow {
    pub fn new_wayland(surface: ToplevelSurface) -> Self {
        let window = Window::new_wayland_window(surface);
        window.user_data().insert_if_missing(|| {
            RefCell::new(FhtWindowData {
                z_index: Arc::new((smithay::desktop::space::RenderZindex::Shell as u32).into()),
                last_floating_geometry: None,
                location: Point::default(),
            })
        });

        FhtWindow(window)
    }

    #[cfg(feature = "xwayland")]
    pub fn new_x11(surface: X11Surface) -> Self {
        let window = Window::new_x11_window(surface);
        window.user_data().insert_if_missing(|| {
            RefCell::new(FhtWindowData {
                z_index: Arc::new((smithay::desktop::space::RenderZindex::Shell as u32).into()),
                last_floating_geometry: None,
                location: Point::default(),
            })
        });

        if window.is_x11() {
            window
                .user_data()
                .insert_if_missing(|| X11SurfaceTiled(false.into()));
        }

        FhtWindow(window)
    }

    pub fn location(&self) -> Point<i32, Global> {
        self.user_data()
            .get::<RefCell<FhtWindowData>>()
            .unwrap()
            .borrow()
            .location
    }

    pub fn render_location(&self) -> Point<i32, Global> {
        self.location() - self.geometry().loc.as_global()
    }

    pub fn global_geometry(&self) -> Rectangle<i32, Global> {
        let mut geo = self.geometry().as_global();
        geo.loc = self.location();
        geo
    }

    pub fn global_bbox(&self) -> Rectangle<i32, Global> {
        let mut bbox = self.bbox().as_global();
        bbox.loc += self.location() - self.geometry().loc.as_global();
        bbox
    }

    pub fn set_geometry(&self, mut geometry: Rectangle<i32, Global>) {
        // Offset for border
        let border_thickness = CONFIG.decoration.border.thickness as i32;
        geometry.loc += (border_thickness, border_thickness).into();
        geometry.size -= (2 * border_thickness, 2 * border_thickness).into();

        if let Some(toplevel) = self.0.toplevel() {
            toplevel.with_pending_state(|s| s.size = Some(geometry.size.as_logical()));
        }
        #[cfg(feature = "xwayland")]
        if let Some(x11_surface) = self.0.x11_surface() {
            let _ = x11_surface.configure(Some(geometry.as_logical()));
        }

        let mut window_data = self
            .user_data()
            .get::<RefCell<FhtWindowData>>()
            .unwrap()
            .borrow_mut();
        window_data.location = geometry.loc;
        if !self.is_tiled() {
            window_data.last_floating_geometry = Some(geometry);
        }
    }

    pub fn is_fullscreen(&self) -> bool {
        if let Some(toplevel) = self.0.toplevel() {
            return toplevel
                .with_pending_state(|s| s.states.contains(XdgToplevelState::Fullscreen));
        }

        #[cfg(feature = "xwayland")]
        if let Some(x11_surface) = self.0.x11_surface() {
            return x11_surface.is_fullscreen();
        }

        unreachable!("What is this window?")
    }

    pub fn set_fullscreen(&self, fullscreen: bool, wl_output: Option<WlOutput>) {
        if let Some(toplevel) = self.0.toplevel() {
            toplevel.with_pending_state(|s| {
                if fullscreen {
                    s.states.set(XdgToplevelState::Fullscreen);
                    s.fullscreen_output = wl_output;
                } else {
                    s.states.unset(XdgToplevelState::Fullscreen);
                    s.fullscreen_output = None;
                }
            });
        }

        #[cfg(feature = "xwayland")]
        if let Some(x11_surface) = self.0.x11_surface() {
            let _ = x11_surface.set_fullscreen(fullscreen);
        }
    }

    pub fn is_maximized(&self) -> bool {
        if let Some(toplevel) = self.0.toplevel() {
            return toplevel.with_pending_state(|s| s.states.contains(XdgToplevelState::Maximized));
        }

        #[cfg(feature = "xwayland")]
        if let Some(x11_surface) = self.0.x11_surface() {
            return x11_surface.is_maximized();
        }

        unreachable!("What is this window?")
    }

    pub fn set_maximized(&self, maximized: bool) {
        if let Some(toplevel) = self.0.toplevel() {
            toplevel.with_pending_state(|s| {
                if maximized {
                    s.states.set(XdgToplevelState::Maximized);
                } else {
                    s.states.unset(XdgToplevelState::Maximized);
                }
            });
        }

        #[cfg(feature = "xwayland")]
        if let Some(x11_surface) = self.0.x11_surface() {
            let _ = x11_surface.set_maximized(maximized);
        }
    }

    pub fn is_tiled(&self) -> bool {
        if let Some(toplevel) = self.0.toplevel() {
            return toplevel.with_pending_state(|s| s.states.contains(XdgToplevelState::TiledLeft));
        }

        #[cfg(feature = "xwayland")]
        if let Some(x11_surface) = self.0.x11_surface() {
            let lock = x11_surface.user_data().get::<X11SurfaceTiled>().unwrap();
            return lock.0.load(std::sync::atomic::Ordering::SeqCst);
        }

        unreachable!("What is this window?")
    }

    pub fn set_tiled(&self, tiled: bool) {
        if let Some(toplevel) = self.0.toplevel() {
            toplevel.with_pending_state(|s| {
                if tiled {
                    s.states.set(XdgToplevelState::TiledLeft)
                } else {
                    s.states.unset(XdgToplevelState::TiledLeft)
                }
            });
        }

        #[cfg(feature = "xwayland")]
        if let Some(x11_surface) = self.0.x11_surface() {
            let lock = x11_surface.user_data().get::<X11SurfaceTiled>().unwrap();
            lock.0.store(tiled, std::sync::atomic::Ordering::SeqCst);
        }

        if !tiled {
            let maybe_last_floating_geometry = self
                .user_data()
                .get::<RefCell<FhtWindowData>>()
                .unwrap()
                .borrow()
                .last_floating_geometry;
            if let Some(last_floating_geometry) = maybe_last_floating_geometry {
                self.set_geometry(last_floating_geometry);
            }
        }
    }

    pub fn set_bounds(&self, bounds: Option<Size<i32, Logical>>) {
        if let Some(toplevel) = self.0.toplevel() {
            toplevel.with_pending_state(|s| s.bounds = bounds)
        }
    }

    pub fn app_id(&self) -> String {
        #[cfg(feature = "xwayland")]
        if let Some(x11_surface) = self.0.x11_surface() {
            return x11_surface.class();
        }

        with_states(self.wl_surface().as_ref().unwrap(), |states| {
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
        #[cfg(feature = "xwayland")]
        if let Some(x11_surface) = self.0.x11_surface() {
            return x11_surface.title();
        }

        with_states(self.wl_surface().as_ref().unwrap(), |states| {
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

    pub fn get_z_index(&self) -> u32 {
        self.user_data()
            .get::<RefCell<FhtWindowData>>()
            .unwrap()
            .borrow()
            .z_index
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_z_index(&self, z_index: u32) {
        self.user_data()
            .get::<RefCell<FhtWindowData>>()
            .unwrap()
            .borrow()
            .z_index
            .store(z_index, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn has_surface(&self, surface: &WlSurface, surface_type: WindowSurfaceType) -> bool {
        let Some(self_surface) = self.wl_surface() else {
            return false;
        };

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

    pub fn surface_under(
        &self,
        point: Point<f64, Logical>,
        surface_type: WindowSurfaceType,
    ) -> Option<(WlSurface, Point<i32, Logical>)> {
        self.0.surface_under(point, surface_type)
    }

    pub fn with_surfaces<F>(&self, processor: F)
    where
        F: FnMut(&WlSurface, &WlSurfaceData),
    {
        self.0.with_surfaces(processor)
    }

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
        self.0
            .send_frame(output, time, throttle, primary_scan_out_output)
    }

    pub fn send_dmabuf_feedback<'a, P, F>(
        &self,
        output: &Output,
        primary_scan_out_output: P,
        select_dmabuf_feedback: F,
    ) where
        P: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
        F: Fn(&WlSurface, &WlSurfaceData) -> &'a DmabufFeedback + Copy,
    {
        self.0
            .send_dmabuf_feedback(output, primary_scan_out_output, select_dmabuf_feedback)
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
        self.0.take_presentation_feedback(
            output_feedback,
            primary_scan_out_output,
            presentation_feedback_flags,
        )
    }

    pub fn close(&self) {
        if let Some(toplevel) = self.0.toplevel() {
            toplevel.send_close();
            return;
        }

        #[cfg(feature = "xwayland")]
        if let Some(x11_surface) = self.0.x11_surface() {
            let _ = x11_surface.close();
        }
    }

    pub fn is_wayland(&self) -> bool {
        self.0.is_wayland()
    }

    #[cfg(feature = "xwayland")]
    pub fn is_x11(&self) -> bool {
        self.0.is_x11()
    }

    #[cfg(feature = "xwayland")]
    pub fn is_x11_override_redirect(&self) -> bool {
        if let Some(x11_surface) = self.0.x11_surface() {
            return x11_surface.is_override_redirect();
        }

        false
    }
    pub fn user_data(&self) -> &UserDataMap {
        self.0.user_data()
    }
}

impl SpaceElement for FhtWindow {
    fn bbox(&self) -> Rectangle<i32, Logical> {
        self.0.bbox()
    }

    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        self.0.is_in_input_region(point)
    }

    fn set_activate(&self, activated: bool) {
        self.0.set_activate(activated)
    }

    fn output_enter(&self, output: &Output, overlap: Rectangle<i32, Logical>) {
        self.0.output_enter(output, overlap)
    }

    fn output_leave(&self, output: &Output) {
        self.0.output_leave(output)
    }

    fn geometry(&self) -> Rectangle<i32, Logical> {
        self.0.geometry()
    }

    fn z_index(&self) -> u8 {
        smithay::desktop::space::RenderZindex::Shell as u8
    }

    fn refresh(&self) {
        self.0.refresh()
    }
}

impl WaylandFocus for FhtWindow {
    fn wl_surface(
        &self,
    ) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
        self.0.wl_surface()
    }

    fn same_client_as(&self, object_id: &wayland_backend::server::ObjectId) -> bool {
        self.0.same_client_as(object_id)
    }
}

impl IsAlive for FhtWindow {
    fn alive(&self) -> bool {
        self.0.alive()
    }
}

impl PointerTarget<State> for FhtWindow {
    fn enter(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::MotionEvent,
    ) {
        PointerTarget::enter(&self.0, seat, data, event)
    }

    fn motion(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::MotionEvent,
    ) {
        self.0.motion(seat, data, event)
    }

    fn relative_motion(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::RelativeMotionEvent,
    ) {
        self.0.relative_motion(seat, data, event)
    }

    fn button(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::ButtonEvent,
    ) {
        self.0.button(seat, data, event)
    }

    fn axis(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        frame: smithay::input::pointer::AxisFrame,
    ) {
        self.0.axis(seat, data, frame)
    }

    fn frame(&self, seat: &Seat<State>, data: &mut State) {
        self.0.frame(seat, data)
    }

    fn gesture_swipe_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GestureSwipeBeginEvent,
    ) {
        self.0.gesture_swipe_begin(seat, data, event)
    }

    fn gesture_swipe_update(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GestureSwipeUpdateEvent,
    ) {
        self.0.gesture_swipe_update(seat, data, event)
    }

    fn gesture_swipe_end(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GestureSwipeEndEvent,
    ) {
        self.0.gesture_swipe_end(seat, data, event)
    }

    fn gesture_pinch_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GesturePinchBeginEvent,
    ) {
        self.0.gesture_pinch_begin(seat, data, event)
    }

    fn gesture_pinch_update(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GesturePinchUpdateEvent,
    ) {
        self.0.gesture_pinch_update(seat, data, event)
    }

    fn gesture_pinch_end(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GesturePinchEndEvent,
    ) {
        self.0.gesture_pinch_end(seat, data, event)
    }

    fn gesture_hold_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GestureHoldBeginEvent,
    ) {
        self.0.gesture_hold_begin(seat, data, event)
    }

    fn gesture_hold_end(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GestureHoldEndEvent,
    ) {
        self.0.gesture_hold_end(seat, data, event)
    }

    fn leave(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        serial: smithay::utils::Serial,
        time: u32,
    ) {
        PointerTarget::leave(&self.0, seat, data, serial, time)
    }
}

impl KeyboardTarget<State> for FhtWindow {
    fn enter(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        keys: Vec<smithay::input::keyboard::KeysymHandle<'_>>,
        serial: smithay::utils::Serial,
    ) {
        KeyboardTarget::enter(&self.0, seat, data, keys, serial)
    }

    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: smithay::utils::Serial) {
        KeyboardTarget::leave(&self.0, seat, data, serial)
    }

    fn key(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        key: smithay::input::keyboard::KeysymHandle<'_>,
        state: smithay::backend::input::KeyState,
        serial: smithay::utils::Serial,
        time: u32,
    ) {
        self.0.key(seat, data, key, state, serial, time)
    }

    fn modifiers(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        modifiers: smithay::input::keyboard::ModifiersState,
        serial: smithay::utils::Serial,
    ) {
        self.0.modifiers(seat, data, modifiers, serial)
    }
}

#[derive(Debug)]
pub enum FhtWindowRenderElement<R>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
{
    Toplevel(WaylandSurfaceRenderElement<R>),
    Subsurface(WaylandSurfaceRenderElement<R>),
    /// A shader element, which is used to render different decorations
    Shader(PixelShaderElement),
}

impl<R> Element for FhtWindowRenderElement<R>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
{
    fn id(&self) -> &smithay::backend::renderer::element::Id {
        match self {
            Self::Toplevel(e) => e.id(),
            Self::Subsurface(e) => e.id(),
            Self::Shader(e) => e.id(),
        }
    }

    fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
        match self {
            Self::Toplevel(e) => e.current_commit(),
            Self::Subsurface(e) => e.current_commit(),
            Self::Shader(e) => e.current_commit(),
        }
    }

    fn src(&self) -> Rectangle<f64, smithay::utils::Buffer> {
        match self {
            Self::Toplevel(e) => e.src(),
            Self::Subsurface(e) => e.src(),
            Self::Shader(e) => e.src(),
        }
    }

    fn geometry(
        &self,
        scale: smithay::utils::Scale<f64>,
    ) -> Rectangle<i32, smithay::utils::Physical> {
        match self {
            Self::Toplevel(e) => e.geometry(scale),
            Self::Subsurface(e) => e.geometry(scale),
            Self::Shader(e) => e.geometry(scale),
        }
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, smithay::utils::Physical> {
        match self {
            Self::Toplevel(e) => e.location(scale),
            Self::Subsurface(e) => e.location(scale),
            Self::Shader(e) => e.location(scale),
        }
    }

    fn transform(&self) -> smithay::utils::Transform {
        match self {
            Self::Toplevel(e) => e.transform(),
            Self::Subsurface(e) => e.transform(),
            Self::Shader(e) => e.transform(),
        }
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<smithay::backend::renderer::utils::CommitCounter>,
    ) -> Vec<Rectangle<i32, smithay::utils::Physical>> {
        match self {
            Self::Toplevel(e) => e.damage_since(scale, commit),
            Self::Subsurface(e) => e.damage_since(scale, commit),
            Self::Shader(e) => e.damage_since(scale, commit),
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, smithay::utils::Physical>> {
        match self {
            Self::Toplevel(e) => {
                // PERF: Write OR code.
                vec![]
                // let or = e.opaque_regions(scale);
                // if or.is_empty() {
                //     return or;
                // }
                // let or = or
                //     .into_iter()
                //     .fold(Rectangle::default(), |acc, r| acc.merge(r));
                // let radius = CONFIG.decoration.border.radius as f64;
                // let size = e.geometry(scale).size.to_f64();
                // vec![
                //     Rectangle::<f64, Physical>::from_extemities(
                //         (0.0, radius),
                //         (size.w, size.h - radius),
                //     )
                //     .to_i32_up()
                //     .intersection(or)
                //     .unwrap_or_default(),
                //     Rectangle::<f64, Physical>::from_extemities(
                //         (radius, 0.0),
                //         (size.w - radius, size.h),
                //     )
                //     .to_i32_up()
                //     .intersection(or)
                //     .unwrap_or_default(),
                // ]
            }
            Self::Subsurface(e) => e.opaque_regions(scale),
            Self::Shader(e) => e.opaque_regions(scale),
        }
    }

    fn alpha(&self) -> f32 {
        match self {
            Self::Toplevel(e) => e.alpha(),
            Self::Subsurface(e) => e.alpha(),
            Self::Shader(e) => e.alpha(),
        }
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlowRenderer> for FhtWindowRenderElement<GlowRenderer> {
    fn draw(
        &self,
        frame: &mut GlowFrame,
        src: Rectangle<f64, smithay::utils::Buffer>,
        dst: Rectangle<i32, smithay::utils::Physical>,
        damage: &[Rectangle<i32, smithay::utils::Physical>],
    ) -> Result<(), GlesError> {
        // DO NOT apply rounded quad shader on subsurface.
        match self {
            Self::Toplevel(e) => {
                let gles_frame: &mut GlesFrame = BorrowMut::borrow_mut(frame);
                let egl_context = gles_frame.egl_context();
                let program = RoundedQuadShader::get(egl_context);

                gles_frame.override_default_tex_program(
                    program,
                    vec![
                        Uniform::new("radius", CONFIG.decoration.border.radius),
                        Uniform::new("size", (dst.size.w as f32, dst.size.h as f32)),
                    ],
                );
                let res = e.draw(frame, src, dst, damage);
                BorrowMut::<GlesFrame>::borrow_mut(frame).clear_tex_program_override();
                res
            }
            Self::Subsurface(e) => e.draw(frame, src, dst, damage),
            Self::Shader(e) => RenderElement::<GlowRenderer>::draw(e, frame, src, dst, damage),
        }
    }

    fn underlying_storage(
        &self,
        renderer: &mut GlowRenderer,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage> {
        match self {
            Self::Toplevel(e) => e.underlying_storage(renderer),
            Self::Subsurface(e) => e.underlying_storage(renderer),
            Self::Shader(e) => e.underlying_storage(renderer),
        }
    }
}

#[cfg(feature = "udev_backend")]
impl<'a> RenderElement<UdevRenderer<'a>> for FhtWindowRenderElement<UdevRenderer<'a>> {
    fn draw<'frame>(
        &self,
        frame: &mut UdevFrame<'a, 'frame>,
        src: Rectangle<f64, smithay::utils::Buffer>,
        dst: Rectangle<i32, smithay::utils::Physical>,
        damage: &[Rectangle<i32, smithay::utils::Physical>],
    ) -> Result<(), UdevRenderError> {
        // Different between Self::Toplevel and Self::Subsurface
        // DO NOT apply rounded quad shader on subsurface.
        match self {
            Self::Toplevel(e) => {
                let gles_frame: &mut GlesFrame = BorrowMut::borrow_mut(frame.glow_frame_mut());
                let egl_context = gles_frame.egl_context();
                let program = RoundedQuadShader::get(egl_context);

                gles_frame.override_default_tex_program(
                    program,
                    vec![
                        Uniform::new("radius", CONFIG.decoration.border.radius),
                        Uniform::new("size", (dst.size.w as f32, dst.size.h as f32)),
                    ],
                );
                let res = e.draw(frame, src, dst, damage);

                BorrowMut::<GlesFrame>::borrow_mut(frame.glow_frame_mut())
                    .clear_tex_program_override();

                res
            }
            Self::Subsurface(e) => e.draw(frame, src, dst, damage),
            Self::Shader(e) => {
                RenderElement::<GlowRenderer>::draw(e, frame.glow_frame_mut(), src, dst, damage)
                    .map_err(|err| UdevRenderError::Render(err))
            }
        }
    }

    fn underlying_storage(
        &self,
        renderer: &mut UdevRenderer<'a>,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage> {
        match self {
            Self::Toplevel(e) => e.underlying_storage(renderer),
            Self::Subsurface(e) => e.underlying_storage(renderer),
            Self::Shader(e) => e.underlying_storage(renderer.glow_renderer_mut()),
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
        is_focused: bool,
        skip_border: bool,
    ) -> Vec<FhtWindowRenderElement<R>>
    where
        R: Renderer + ImportAll + AsGlowRenderer,
        <R as Renderer>::TextureId: 'static,
        WaylandSurfaceRenderElement<R>: RenderElement<R>,
    {
        let mut render_elements = vec![];
        let Some(wl_surface) = self.wl_surface() else {
            return render_elements;
        };

        let location = self.global_geometry().loc.as_logical();
        let render_location = self
            .render_location()
            .as_logical()
            .to_physical_precise_round(scale);

        let popup_render_elements = PopupManager::popups_for_surface(&wl_surface)
            .flat_map(|(p, offset)| {
                let offset = (self.geometry().loc + offset - p.geometry().loc)
                    .to_physical_precise_round(scale);

                render_elements_from_surface_tree(
                    renderer,
                    p.wl_surface(),
                    render_location + offset,
                    scale,
                    alpha,
                    Kind::Unspecified,
                )
            })
            .map(FhtWindowRenderElement::Subsurface);
        render_elements.extend(popup_render_elements);

        if !skip_border {
            let border_config = &CONFIG.decoration.border;
            let settings = RoundedOutlineShaderSettings {
                thickness: border_config.thickness,
                radius: border_config.radius,
                color: if is_focused {
                    border_config.focused_color
                } else {
                    border_config.normal_color
                },
            };
            let element = RoundedOutlineShader::element(
                renderer,
                scale.x.max(scale.y), // WARN: This may not be accurate.
                alpha,
                &wl_surface,
                Rectangle::from_loc_and_size(location.as_local(), self.geometry().size.as_local()),
                settings,
            );
            render_elements.push(FhtWindowRenderElement::Shader(element));
        }

        let window_render_elements = render_elements_from_surface_tree(
            renderer,
            &wl_surface,
            render_location,
            scale,
            alpha,
            Kind::Unspecified,
        )
        .into_iter()
        .map(FhtWindowRenderElement::Toplevel);
        render_elements.extend(window_render_elements);

        render_elements
    }
}
