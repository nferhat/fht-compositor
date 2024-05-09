use smithay::backend::input::KeyState;
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::Uniform;
use smithay::desktop::space::{RenderZindex, SpaceElement};
use smithay::desktop::{PopupManager, Window, WindowSurface};
use smithay::input::keyboard::{KeyboardTarget, KeysymHandle, ModifiersState};
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
    GestureSwipeUpdateEvent, MotionEvent as PointerMotionEvent, PointerTarget, RelativeMotionEvent,
};
use smithay::input::touch::{
    DownEvent, MotionEvent as TouchMotionEvent, OrientationEvent, ShapeEvent, TouchTarget, UpEvent,
};
use smithay::input::Seat;
use smithay::output::Output;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Serial};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::ToplevelSurface;

use crate::renderer::custom_texture_shader_element::CustomTextureShaderElement;
use crate::renderer::rounded_quad_shader::RoundedQuadShader;
use crate::renderer::FhtRenderer;
use crate::state::State;

/// A window surface.
///
/// The window surface is the actual window, with no decorations/effects applied to it. Something
/// like FhtWindow is responsible for drawing the borders and animating the surface, aswell as
/// managing the surface's properties like location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FhtWindowSurface {
    pub(crate) inner: Window,
}

impl FhtWindowSurface {
    pub fn toplevel(&self) -> &ToplevelSurface {
        self.inner.toplevel().expect("We do not support Xwayland.")
    }
}

impl SpaceElement for FhtWindowSurface {
    fn bbox(&self) -> Rectangle<i32, Logical> {
        self.inner.bbox()
    }

    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        self.inner.is_in_input_region(point)
    }

    fn set_activate(&self, activated: bool) {
        self.inner.set_activate(activated)
    }

    fn output_enter(&self, output: &Output, overlap: Rectangle<i32, Logical>) {
        self.inner.output_enter(output, overlap)
    }

    fn output_leave(&self, output: &Output) {
        self.inner.output_leave(output)
    }

    fn geometry(&self) -> Rectangle<i32, Logical> {
        self.inner.geometry()
    }

    fn z_index(&self) -> u8 {
        RenderZindex::Shell as u8
    }

    fn refresh(&self) {
        self.inner.refresh()
    }
}

impl WaylandFocus for FhtWindowSurface {
    fn wl_surface(&self) -> Option<WlSurface> {
        self.inner.wl_surface()
    }

    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        self.inner.same_client_as(object_id)
    }
}

impl IsAlive for FhtWindowSurface {
    fn alive(&self) -> bool {
        self.inner.alive()
    }
}

impl PointerTarget<State> for FhtWindowSurface {
    fn enter(&self, seat: &Seat<State>, data: &mut State, event: &PointerMotionEvent) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => PointerTarget::enter(w.wl_surface(), seat, data, event),
        }
    }

    fn motion(&self, seat: &Seat<State>, data: &mut State, event: &PointerMotionEvent) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => PointerTarget::motion(w.wl_surface(), seat, data, event),
        }
    }

    fn relative_motion(&self, seat: &Seat<State>, data: &mut State, event: &RelativeMotionEvent) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::relative_motion(w.wl_surface(), seat, data, event)
            }
        }
    }

    fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => PointerTarget::button(w.wl_surface(), seat, data, event),
        }
    }

    fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => PointerTarget::axis(w.wl_surface(), seat, data, frame),
        }
    }

    fn frame(&self, seat: &Seat<State>, data: &mut State) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => PointerTarget::frame(w.wl_surface(), seat, data),
        }
    }

    fn gesture_swipe_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GestureSwipeBeginEvent,
    ) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_swipe_begin(w.wl_surface(), seat, data, event)
            }
        }
    }

    fn gesture_swipe_update(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GestureSwipeUpdateEvent,
    ) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_swipe_update(w.wl_surface(), seat, data, event)
            }
        }
    }

    fn gesture_swipe_end(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GestureSwipeEndEvent,
    ) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_swipe_end(w.wl_surface(), seat, data, event)
            }
        }
    }

    fn gesture_pinch_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GesturePinchBeginEvent,
    ) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_pinch_begin(w.wl_surface(), seat, data, event)
            }
        }
    }

    fn gesture_pinch_update(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GesturePinchUpdateEvent,
    ) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_pinch_update(w.wl_surface(), seat, data, event)
            }
        }
    }

    fn gesture_pinch_end(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GesturePinchEndEvent,
    ) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_pinch_end(w.wl_surface(), seat, data, event)
            }
        }
    }

    fn gesture_hold_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GestureHoldBeginEvent,
    ) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_hold_begin(w.wl_surface(), seat, data, event)
            }
        }
    }

    fn gesture_hold_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldEndEvent) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::gesture_hold_end(w.wl_surface(), seat, data, event)
            }
        }
    }

    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                PointerTarget::leave(w.wl_surface(), seat, data, serial, time)
            }
        }
    }
}

impl TouchTarget<State> for FhtWindowSurface {
    fn down(&self, seat: &Seat<State>, data: &mut State, event: &DownEvent, seq: Serial) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => TouchTarget::down(w.wl_surface(), seat, data, event, seq),
        }
    }

    fn up(&self, seat: &Seat<State>, data: &mut State, event: &UpEvent, seq: Serial) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => TouchTarget::up(w.wl_surface(), seat, data, event, seq),
        }
    }

    fn motion(&self, seat: &Seat<State>, data: &mut State, event: &TouchMotionEvent, seq: Serial) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                TouchTarget::motion(w.wl_surface(), seat, data, event, seq)
            }
        }
    }

    fn frame(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => TouchTarget::frame(w.wl_surface(), seat, data, seq),
        }
    }

    fn cancel(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => TouchTarget::cancel(w.wl_surface(), seat, data, seq),
        }
    }

    fn shape(&self, seat: &Seat<State>, data: &mut State, event: &ShapeEvent, seq: Serial) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => TouchTarget::shape(w.wl_surface(), seat, data, event, seq),
        }
    }

    fn orientation(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &OrientationEvent,
        seq: Serial,
    ) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                TouchTarget::orientation(w.wl_surface(), seat, data, event, seq)
            }
        }
    }
}

impl KeyboardTarget<State> for FhtWindowSurface {
    fn enter(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                KeyboardTarget::enter(w.wl_surface(), seat, data, keys, serial)
            }
        }
    }

    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => KeyboardTarget::leave(w.wl_surface(), seat, data, serial),
        }
    }

    fn key(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                KeyboardTarget::key(w.wl_surface(), seat, data, key, state, serial, time)
            }
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match self.inner.underlying_surface() {
            WindowSurface::Wayland(w) => {
                KeyboardTarget::modifiers(w.wl_surface(), seat, data, modifiers, serial)
            }
        }
    }
}

pub type FhtWindowSurfaceRenderElement<R> =
    CustomTextureShaderElement<WaylandSurfaceRenderElement<R>>;

impl FhtWindowSurface {
    #[profiling::function]
    pub(super) fn render_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
        border_radius: Option<f32>,
    ) -> Vec<FhtWindowSurfaceRenderElement<R>> {
        let surface = self.toplevel().wl_surface();

        let mut render_elements = PopupManager::popups_for_surface(surface)
            .flat_map(|(popup, popup_offset)| {
                let offset = (self.geometry().loc + popup_offset - popup.geometry().loc)
                    .to_physical_precise_round(scale);

                render_elements_from_surface_tree(
                    renderer,
                    popup.wl_surface(),
                    location + offset,
                    scale,
                    alpha,
                    Kind::Unspecified,
                )
                .into_iter()
                .map(FhtWindowSurfaceRenderElement::from_element_no_shader)
            })
            .collect::<Vec<_>>();

        render_elements.extend(
            render_elements_from_surface_tree(
                renderer,
                surface,
                location,
                scale,
                alpha,
                Kind::Unspecified,
            )
            .into_iter()
            .map(|e| {
                if let Some(border_radius) = border_radius {
                    let texture_shader = RoundedQuadShader::get(renderer);
                    FhtWindowSurfaceRenderElement::from_element(
                        e,
                        texture_shader,
                        vec![Uniform::new("radius", border_radius)],
                    )
                } else {
                    FhtWindowSurfaceRenderElement::from_element_no_shader(e)
                }
            }),
        );

        render_elements
    }
}
