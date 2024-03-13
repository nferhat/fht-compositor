//! A custom shell window.
//!
//! [`FhtWindow`] is a wrapper around the [`Window`] type provided by Smithay. It allows me to add
//! some abstractions around state for this compositor stored in [`FhtWindowData`].
//!
//! The window is self-contained, meaning that, independent from how you use it, has it's own
//! geometry (loc+size), border, rendering, etc.

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
use smithay::backend::renderer::utils::DamageSet;
use smithay::backend::renderer::{ImportAll, Renderer};
use smithay::desktop::space::SpaceElement;
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::desktop::{PopupManager, Window, WindowSurface, WindowSurfaceType};
use smithay::input::keyboard::KeyboardTarget;
use smithay::input::pointer::PointerTarget;
use smithay::input::touch::TouchTarget;
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

/// Additional compositor-specific data to store inside a [`Window`] user data map.
pub struct FhtWindowData {
    /// NOTE: The window doesn't manage this in any way, it's up to the workspace it's on to manage
    /// the Z-index relative to other windows.
    pub z_index: Arc<AtomicU32>,

    pub location: Point<i32, Global>,
    pub last_floating_geometry: Option<Rectangle<i32, Global>>,
}

pub type FhtWindowUserData = RefCell<FhtWindowData>;

/// An abstraction over Smithay's builtin [`Window`] type.
#[derive(Debug, Clone, PartialEq, Hash)]
pub struct FhtWindow(pub Window);

/// Since [`X11Surface`]s don't have a way to store whether they are tiled or not unlike wayland
/// windows that do through the xdg-shell protocol
#[cfg(feature = "xwayland")]
pub struct X11SurfaceTiled(AtomicBool);

impl FhtWindow {
    /// Create a new Wayland-based window using the xdg_shell protocol.
    pub fn new_wayland(surface: ToplevelSurface) -> Self {
        let window = Window::new_wayland_window(surface);
        window.user_data().insert_if_missing(|| {
            FhtWindowUserData::new(FhtWindowData {
                z_index: Arc::new((smithay::desktop::space::RenderZindex::Shell as u32).into()),
                last_floating_geometry: None,
                location: Point::default(),
            })
        });

        FhtWindow(window)
    }

    /// Create a new X11 window managed by the running Xwayland/X11wm instance.
    #[cfg(feature = "xwayland")]
    pub fn new_x11(surface: X11Surface) -> Self {
        let window = Window::new_x11_window(surface);
        window.user_data().insert_if_missing(|| {
            FhtWindowUserData::new(FhtWindowData {
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

    /// Get the global location of the window.
    pub fn location(&self) -> Point<i32, Global> {
        self.user_data()
            .get::<FhtWindowUserData>()
            .unwrap()
            .borrow()
            .location
    }

    /// Get the render location of this window. This may or may not be the same as it's location.
    ///
    /// The different is that the render location offsets for decorations (shadows, titlebars) that
    /// the window may set.
    pub fn render_location(&self) -> Point<i32, Global> {
        self.location() - self.geometry().loc.as_global()
    }

    /// Get the global geometry of this window, size falling back to the gloal bbox of the window.
    ///
    /// NOTE: The rectangle location is from [`Self::location`]
    pub fn global_geometry(&self) -> Rectangle<i32, Global> {
        let mut geo = self.geometry().as_global();
        geo.loc = self.location();
        geo
    }

    /// Get the global bounding box of this window.
    ///
    /// Almost the same as [`Self::global_geometry`], but the size wraps around the whole window
    /// and it's subsurfaces.
    pub fn global_bbox(&self) -> Rectangle<i32, Global> {
        let mut bbox = self.bbox().as_global();
        bbox.loc += self.location() - self.geometry().loc.as_global();
        bbox
    }

    /// Set the geometry of this window.
    ///
    /// You can use `remove_border` to account for this window border size.
    pub fn set_geometry(&self, mut geometry: Rectangle<i32, Global>, remove_border: bool) {
        if remove_border {
            let border_thickness = CONFIG.decoration.border.thickness as i32;
            geometry.loc += (border_thickness, border_thickness).into();
            geometry.size -= (2 * border_thickness, 2 * border_thickness).into();
        }

        if let Some(toplevel) = self.0.toplevel() {
            toplevel.with_pending_state(|s| s.size = Some(geometry.size.as_logical()));
        }
        #[cfg(feature = "xwayland")]
        if let Some(x11_surface) = self.0.x11_surface() {
            let _ = x11_surface.configure(Some(geometry.as_logical()));
        }

        let mut window_data = self
            .user_data()
            .get::<FhtWindowUserData>()
            .unwrap()
            .borrow_mut();
        window_data.location = geometry.loc;
        if !self.is_tiled() {
            window_data.last_floating_geometry = Some(geometry);
        }
    }

    /// Return whether the window is fullscreened or not.
    ///
    /// NOTE: This returns the pending state.
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

    /// Set whether the window is fullscreened or not.
    ///
    /// NOTE: In case this window is a Wayland window, you still have to send a configure message
    /// to the underyling [`ToplevelSurface`]
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

    /// Return whether the window is maximized or not.
    ///
    /// NOTE: This returns the pending state.
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

    /// Set whether the window is maximized or not.
    ///
    /// NOTE: In case this window is a Wayland window, you still have to send a configure message
    /// to the underyling [`ToplevelSurface`]
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

    /// Return whether the window is tiled or not.
    ///
    /// NOTE: This returns the pending state.
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

    /// Set whether the window is tiled or not.
    ///
    /// Setting this to true will also (for some windows) disable client side decorations such as
    /// shadows.
    ///
    /// NOTE: In case this window is a Wayland window, you still have to send a configure message
    /// to the underyling [`ToplevelSurface`]
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
                .get::<FhtWindowUserData>()
                .unwrap()
                .borrow()
                .last_floating_geometry;
            if let Some(last_floating_geometry) = maybe_last_floating_geometry {
                self.set_geometry(last_floating_geometry, false);
            }
        }
    }

    /// Set the bounds of this window
    ///
    /// NOTE: This only affects Wayland windows
    /// NOTE: You still have to send a configure message to the underyling [`ToplevelSurface`]
    pub fn set_bounds(&self, bounds: Option<Size<i32, Logical>>) {
        if let Some(toplevel) = self.0.toplevel() {
            toplevel.with_pending_state(|s| s.bounds = bounds)
        }
    }

    /// Return the app_id of this window.
    ///
    /// This also has other common name such as the window class, or the WM_CLASS atom on X11
    /// windows.
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

    /// Return the title of this window.
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

    /// Return the Z-index of this window.
    ///
    /// This literally does NOTHING if whatever you are managing the window with (for example your
    /// workspace, or something...) does not account for this.
    pub fn get_z_index(&self) -> u32 {
        self.user_data()
            .get::<FhtWindowUserData>()
            .unwrap()
            .borrow()
            .z_index
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Set the Z-index of this window.
    pub fn set_z_index(&self, z_index: u32) {
        self.user_data()
            .get::<FhtWindowUserData>()
            .unwrap()
            .borrow()
            .z_index
            .store(z_index, std::sync::atomic::Ordering::SeqCst);
    }

    /// Return whether this window owns this [`WlSurface`] with surface type [`WindowSurfaceType`]
    ///
    /// You can with this check if a window owns a popup, for example.
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

    /// Return the topmost surface owned by this window under this point.
    ///
    /// NOTE: This function expects `point` to be relative to the window origin. You can achieve
    /// this by offseting it by [`Self::render_location`]
    pub fn surface_under(
        &self,
        point: Point<f64, Logical>,
        surface_type: WindowSurfaceType,
    ) -> Option<(WlSurface, Point<i32, Logical>)> {
        self.0.surface_under(point, surface_type)
    }

    /// Run a closure on all the window surfaces.
    pub fn with_surfaces<F>(&self, processor: F)
    where
        F: FnMut(&WlSurface, &WlSurfaceData),
    {
        self.0.with_surfaces(processor)
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
        self.0
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

    /// Close this window.
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

    /// Return whether this window is a Wayland window.
    pub fn is_wayland(&self) -> bool {
        self.0.is_wayland()
    }

    /// Return whether this window is a X11 window.
    #[cfg(feature = "xwayland")]
    pub fn is_x11(&self) -> bool {
        self.0.is_x11()
    }

    /// Return whether this window is a X11 override-redirect window.
    #[cfg(feature = "xwayland")]
    pub fn is_x11_override_redirect(&self) -> bool {
        if let Some(x11_surface) = self.0.x11_surface() {
            return x11_surface.is_override_redirect();
        }

        false
    }

    /// Get access to this window [`UserDataMap`]
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
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => PointerTarget::enter(w.wl_surface(), seat, data, event),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::enter(x, seat, data, event),
        }
    }

    fn motion(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::MotionEvent,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => PointerTarget::motion(w.wl_surface(), seat, data, event),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::motion(x, seat, data, event),
        }
    }

    fn relative_motion(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::RelativeMotionEvent,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::relative_motion(w.wl_surface(), seat, data, event)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::relative_motion(x, seat, data, event),
        }
    }

    fn button(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::ButtonEvent,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => PointerTarget::button(w.wl_surface(), seat, data, event),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::button(x, seat, data, event),
        }
    }

    fn axis(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        frame: smithay::input::pointer::AxisFrame,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => PointerTarget::axis(w.wl_surface(), seat, data, frame),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::axis(x, seat, data, frame),
        }
    }

    fn frame(&self, seat: &Seat<State>, data: &mut State) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => PointerTarget::frame(w.wl_surface(), seat, data),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::frame(x, seat, data),
        }
    }

    fn gesture_swipe_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GestureSwipeBeginEvent,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_swipe_begin(w.wl_surface(), seat, data, event)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::gesture_swipe_begin(x, seat, data, event),
        }
    }

    fn gesture_swipe_update(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GestureSwipeUpdateEvent,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_swipe_update(w.wl_surface(), seat, data, event)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::gesture_swipe_update(x, seat, data, event),
        }
    }

    fn gesture_swipe_end(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GestureSwipeEndEvent,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_swipe_end(w.wl_surface(), seat, data, event)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::gesture_swipe_end(x, seat, data, event),
        }
    }

    fn gesture_pinch_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GesturePinchBeginEvent,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_pinch_begin(w.wl_surface(), seat, data, event)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::gesture_pinch_begin(x, seat, data, event),
        }
    }

    fn gesture_pinch_update(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GesturePinchUpdateEvent,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_pinch_update(w.wl_surface(), seat, data, event)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::gesture_pinch_update(x, seat, data, event),
        }
    }

    fn gesture_pinch_end(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GesturePinchEndEvent,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_pinch_end(w.wl_surface(), seat, data, event)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::gesture_pinch_end(x, seat, data, event),
        }
    }

    fn gesture_hold_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GestureHoldBeginEvent,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_hold_begin(w.wl_surface(), seat, data, event)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::gesture_hold_begin(x, seat, data, event),
        }
    }

    fn gesture_hold_end(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::pointer::GestureHoldEndEvent,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_hold_end(w.wl_surface(), seat, data, event)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::gesture_hold_end(x, seat, data, event),
        }
    }

    fn leave(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        serial: smithay::utils::Serial,
        time: u32,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::leave(w.wl_surface(), seat, data, serial, time)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => PointerTarget::leave(x, seat, data, serial, time),
        }
    }
}

impl TouchTarget<State> for FhtWindow {
    fn down(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::touch::DownEvent,
        seq: smithay::utils::Serial,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => TouchTarget::down(w.wl_surface(), seat, data, event, seq),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => TouchTarget::down(x, seat, data, event, seq),
        }
    }

    fn up(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::touch::UpEvent,
        seq: smithay::utils::Serial,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => TouchTarget::up(w.wl_surface(), seat, data, event, seq),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => TouchTarget::up(x, seat, data, event, seq),
        }
    }

    fn motion(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::touch::MotionEvent,
        seq: smithay::utils::Serial,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                TouchTarget::motion(w.wl_surface(), seat, data, event, seq)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => TouchTarget::motion(x, seat, data, event, seq),
        }
    }

    fn frame(&self, seat: &Seat<State>, data: &mut State, seq: smithay::utils::Serial) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => TouchTarget::frame(w.wl_surface(), seat, data, seq),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => TouchTarget::frame(x, seat, data, seq),
        }
    }

    fn cancel(&self, seat: &Seat<State>, data: &mut State, seq: smithay::utils::Serial) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => TouchTarget::cancel(w.wl_surface(), seat, data, seq),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => TouchTarget::cancel(x, seat, data, seq),
        }
    }

    fn shape(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::touch::ShapeEvent,
        seq: smithay::utils::Serial,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => TouchTarget::shape(w.wl_surface(), seat, data, event, seq),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => TouchTarget::shape(x, seat, data, event, seq),
        }
    }

    fn orientation(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::touch::OrientationEvent,
        seq: smithay::utils::Serial,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                TouchTarget::orientation(w.wl_surface(), seat, data, event, seq)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => TouchTarget::orientation(x, seat, data, event, seq),
        }
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
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                KeyboardTarget::enter(w.wl_surface(), seat, data, keys, serial)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => KeyboardTarget::enter(x, seat, data, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: smithay::utils::Serial) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => KeyboardTarget::leave(w.wl_surface(), seat, data, serial),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => KeyboardTarget::leave(x, seat, data, serial),
        }
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
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                KeyboardTarget::key(w.wl_surface(), seat, data, key, state, serial, time)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => KeyboardTarget::key(x, seat, data, key, state, serial, time),
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        modifiers: smithay::input::keyboard::ModifiersState,
        serial: smithay::utils::Serial,
    ) {
        match self.0.underlying_surface() {
            WindowSurface::Wayland(w) => {
                KeyboardTarget::modifiers(w.wl_surface(), seat, data, modifiers, serial)
            }
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(x) => KeyboardTarget::modifiers(x, seat, data, modifiers, serial),
        }
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
    ) -> DamageSet<i32, Physical> {
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
