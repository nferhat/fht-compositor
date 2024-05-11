pub use smithay::backend::input::KeyState;
use smithay::desktop::Window;
pub use smithay::desktop::{LayerSurface, PopupKind};
pub use smithay::input::keyboard::{KeyboardTarget, KeysymHandle, ModifiersState};
pub use smithay::input::pointer::{
    AxisFrame, ButtonEvent, MotionEvent, PointerTarget, RelativeMotionEvent,
};
use smithay::input::pointer::{
    GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
    GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
};
use smithay::input::touch::TouchTarget;
use smithay::input::Seat;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Serial};
use smithay::wayland::seat::WaylandFocus;
use smithay_egui::EguiState;

use crate::state::State;

#[derive(Clone, Debug, PartialEq)]
pub enum KeyboardFocusTarget {
    Window(Window),
    LayerSurface(LayerSurface),
    Popup(PopupKind),
}

impl From<Window> for KeyboardFocusTarget {
    fn from(value: Window) -> Self {
        Self::Window(value)
    }
}

impl From<LayerSurface> for KeyboardFocusTarget {
    fn from(value: LayerSurface) -> Self {
        Self::LayerSurface(value)
    }
}

impl From<PopupKind> for KeyboardFocusTarget {
    fn from(value: PopupKind) -> Self {
        Self::Popup(value)
    }
}

impl WaylandFocus for KeyboardFocusTarget {
    fn wl_surface(&self) -> Option<WlSurface> {
        match self {
            Self::Window(w) => w.wl_surface(),
            Self::LayerSurface(l) => Some(l.wl_surface().clone()),
            Self::Popup(p) => Some(p.wl_surface().clone()),
        }
    }

    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            Self::Window(w) => w.same_client_as(object_id),
            Self::LayerSurface(l) => l.same_client_as(object_id),
            Self::Popup(p) => p.wl_surface().same_client_as(object_id),
        }
    }
}

impl IsAlive for KeyboardFocusTarget {
    fn alive(&self) -> bool {
        match self {
            Self::Window(w) => w.alive(),
            Self::LayerSurface(l) => l.alive(),
            Self::Popup(p) => p.alive(),
        }
    }
}

impl KeyboardTarget<State> for KeyboardFocusTarget {
    fn enter(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            Self::Window(w) => {
                KeyboardTarget::enter(w.toplevel().unwrap().wl_surface(), seat, data, keys, serial)
            }
            Self::LayerSurface(l) => {
                KeyboardTarget::enter(l.wl_surface(), seat, data, keys, serial)
            }
            Self::Popup(p) => KeyboardTarget::enter(p.wl_surface(), seat, data, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {
        match self {
            Self::Window(w) => {
                KeyboardTarget::leave(w.toplevel().unwrap().wl_surface(), seat, data, serial)
            }
            Self::LayerSurface(l) => KeyboardTarget::leave(l.wl_surface(), seat, data, serial),
            Self::Popup(p) => KeyboardTarget::leave(p.wl_surface(), seat, data, serial),
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
        match self {
            Self::Window(w) => KeyboardTarget::key(
                w.toplevel().unwrap().wl_surface(),
                seat,
                data,
                key,
                state,
                serial,
                time,
            ),
            Self::LayerSurface(l) => {
                KeyboardTarget::key(l.wl_surface(), seat, data, key, state, serial, time)
            }
            Self::Popup(p) => {
                KeyboardTarget::key(p.wl_surface(), seat, data, key, state, serial, time)
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
        match self {
            Self::Window(w) => KeyboardTarget::modifiers(
                w.toplevel().unwrap().wl_surface(),
                seat,
                data,
                modifiers,
                serial,
            ),
            Self::LayerSurface(l) => {
                KeyboardTarget::modifiers(l.wl_surface(), seat, data, modifiers, serial)
            }
            Self::Popup(p) => {
                KeyboardTarget::modifiers(p.wl_surface(), seat, data, modifiers, serial)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PointerFocusTarget {
    WlSurface(WlSurface),
    Window(Window),
    Egui(EguiState),
}

impl From<KeyboardFocusTarget> for PointerFocusTarget {
    fn from(value: KeyboardFocusTarget) -> Self {
        match value {
            KeyboardFocusTarget::Window(w) => Self::Window(w),
            KeyboardFocusTarget::LayerSurface(surface) => {
                PointerFocusTarget::from(surface.wl_surface().clone())
            }
            KeyboardFocusTarget::Popup(popup) => {
                PointerFocusTarget::from(popup.wl_surface().clone())
            }
        }
    }
}

impl From<WlSurface> for PointerFocusTarget {
    fn from(value: WlSurface) -> Self {
        Self::WlSurface(value)
    }
}

impl From<LayerSurface> for PointerFocusTarget {
    fn from(value: LayerSurface) -> Self {
        Self::WlSurface(value.wl_surface().clone())
    }
}

impl From<EguiState> for PointerFocusTarget {
    fn from(value: EguiState) -> Self {
        Self::Egui(value)
    }
}

impl From<Window> for PointerFocusTarget {
    fn from(value: Window) -> Self {
        Self::Window(value)
    }
}

impl WaylandFocus for PointerFocusTarget {
    fn wl_surface(&self) -> Option<WlSurface> {
        match self {
            Self::WlSurface(w) => w.wl_surface(),
            Self::Window(w) => w.wl_surface(),
            Self::Egui(_) => None,
        }
    }
    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            Self::WlSurface(w) => w.same_client_as(object_id),
            Self::Window(w) => w.same_client_as(object_id),
            Self::Egui(_) => false,
        }
    }
}

impl IsAlive for PointerFocusTarget {
    fn alive(&self) -> bool {
        match self {
            Self::WlSurface(w) => w.alive(),
            Self::Window(w) => w.alive(),
            Self::Egui(e) => e.alive(),
        }
    }
}

impl PointerTarget<State> for PointerFocusTarget {
    fn enter(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {
        match self {
            Self::WlSurface(w) => PointerTarget::enter(w, seat, data, event),
            Self::Window(w) => {
                PointerTarget::enter(w.toplevel().unwrap().wl_surface(), seat, data, event)
            }
            Self::Egui(e) => PointerTarget::enter(e, seat, data, event),
        }
    }

    fn motion(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {
        match self {
            Self::WlSurface(w) => PointerTarget::motion(w, seat, data, event),
            Self::Window(w) => {
                PointerTarget::motion(w.toplevel().unwrap().wl_surface(), seat, data, event)
            }
            Self::Egui(e) => PointerTarget::motion(e, seat, data, event),
        }
    }

    fn relative_motion(&self, seat: &Seat<State>, data: &mut State, event: &RelativeMotionEvent) {
        match self {
            Self::WlSurface(w) => PointerTarget::relative_motion(w, seat, data, event),
            Self::Window(w) => PointerTarget::relative_motion(
                w.toplevel().unwrap().wl_surface(),
                seat,
                data,
                event,
            ),
            Self::Egui(e) => PointerTarget::relative_motion(e, seat, data, event),
        }
    }

    fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {
        match self {
            Self::WlSurface(w) => PointerTarget::button(w, seat, data, event),
            Self::Window(w) => {
                PointerTarget::button(w.toplevel().unwrap().wl_surface(), seat, data, event)
            }
            Self::Egui(e) => PointerTarget::button(e, seat, data, event),
        }
    }

    fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {
        match self {
            Self::WlSurface(w) => PointerTarget::axis(w, seat, data, frame),
            Self::Window(w) => {
                PointerTarget::axis(w.toplevel().unwrap().wl_surface(), seat, data, frame)
            }
            Self::Egui(e) => PointerTarget::axis(e, seat, data, frame),
        }
    }

    fn frame(&self, seat: &Seat<State>, data: &mut State) {
        match self {
            Self::WlSurface(w) => PointerTarget::frame(w, seat, data),
            Self::Window(w) => PointerTarget::frame(w.toplevel().unwrap().wl_surface(), seat, data),
            Self::Egui(e) => PointerTarget::frame(e, seat, data),
        }
    }

    fn gesture_swipe_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GestureSwipeBeginEvent,
    ) {
        match self {
            Self::WlSurface(w) => PointerTarget::gesture_swipe_begin(w, seat, data, event),
            Self::Window(w) => PointerTarget::gesture_swipe_begin(
                w.toplevel().unwrap().wl_surface(),
                seat,
                data,
                event,
            ),
            Self::Egui(e) => PointerTarget::gesture_swipe_begin(e, seat, data, event),
        }
    }

    fn gesture_swipe_update(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GestureSwipeUpdateEvent,
    ) {
        match self {
            Self::WlSurface(w) => PointerTarget::gesture_swipe_update(w, seat, data, event),
            Self::Window(w) => PointerTarget::gesture_swipe_update(
                w.toplevel().unwrap().wl_surface(),
                seat,
                data,
                event,
            ),
            Self::Egui(e) => PointerTarget::gesture_swipe_update(e, seat, data, event),
        }
    }

    fn gesture_swipe_end(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GestureSwipeEndEvent,
    ) {
        match self {
            Self::WlSurface(w) => PointerTarget::gesture_swipe_end(w, seat, data, event),
            Self::Window(w) => PointerTarget::gesture_swipe_end(
                w.toplevel().unwrap().wl_surface(),
                seat,
                data,
                event,
            ),
            Self::Egui(e) => PointerTarget::gesture_swipe_end(e, seat, data, event),
        }
    }

    fn gesture_pinch_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GesturePinchBeginEvent,
    ) {
        match self {
            Self::WlSurface(w) => PointerTarget::gesture_pinch_begin(w, seat, data, event),
            Self::Window(w) => PointerTarget::gesture_pinch_begin(
                w.toplevel().unwrap().wl_surface(),
                seat,
                data,
                event,
            ),
            Self::Egui(e) => PointerTarget::gesture_pinch_begin(e, seat, data, event),
        }
    }

    fn gesture_pinch_update(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GesturePinchUpdateEvent,
    ) {
        match self {
            Self::WlSurface(w) => PointerTarget::gesture_pinch_update(w, seat, data, event),
            Self::Window(w) => PointerTarget::gesture_pinch_update(
                w.toplevel().unwrap().wl_surface(),
                seat,
                data,
                event,
            ),
            Self::Egui(e) => PointerTarget::gesture_pinch_update(e, seat, data, event),
        }
    }

    fn gesture_pinch_end(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GesturePinchEndEvent,
    ) {
        match self {
            Self::WlSurface(w) => PointerTarget::gesture_pinch_end(w, seat, data, event),
            Self::Window(w) => PointerTarget::gesture_pinch_end(
                w.toplevel().unwrap().wl_surface(),
                seat,
                data,
                event,
            ),
            Self::Egui(e) => PointerTarget::gesture_pinch_end(e, seat, data, event),
        }
    }

    fn gesture_hold_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GestureHoldBeginEvent,
    ) {
        match self {
            Self::WlSurface(w) => PointerTarget::gesture_hold_begin(w, seat, data, event),
            Self::Window(w) => PointerTarget::gesture_hold_begin(
                w.toplevel().unwrap().wl_surface(),
                seat,
                data,
                event,
            ),
            Self::Egui(e) => PointerTarget::gesture_hold_begin(e, seat, data, event),
        }
    }

    fn gesture_hold_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldEndEvent) {
        match self {
            Self::WlSurface(w) => PointerTarget::gesture_hold_end(w, seat, data, event),
            Self::Window(w) => PointerTarget::gesture_hold_end(
                w.toplevel().unwrap().wl_surface(),
                seat,
                data,
                event,
            ),
            Self::Egui(e) => PointerTarget::gesture_hold_end(e, seat, data, event),
        }
    }

    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {
        match self {
            Self::WlSurface(w) => PointerTarget::leave(w, seat, data, serial, time),
            Self::Window(w) => {
                PointerTarget::leave(w.toplevel().unwrap().wl_surface(), seat, data, serial, time)
            }
            Self::Egui(e) => PointerTarget::leave(e, seat, data, serial, time),
        }
    }
}

impl TouchTarget<State> for PointerFocusTarget {
    fn down(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::touch::DownEvent,
        seq: Serial,
    ) {
        match self {
            Self::WlSurface(w) => TouchTarget::down(w, seat, data, event, seq),
            Self::Window(w) => {
                TouchTarget::down(w.toplevel().unwrap().wl_surface(), seat, data, event, seq)
            }
            Self::Egui(_) => (),
        }
    }

    fn up(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::touch::UpEvent,
        seq: Serial,
    ) {
        match self {
            Self::WlSurface(w) => TouchTarget::up(w, seat, data, event, seq),
            Self::Window(w) => {
                TouchTarget::up(w.toplevel().unwrap().wl_surface(), seat, data, event, seq)
            }
            Self::Egui(_) => (),
        }
    }

    fn motion(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::touch::MotionEvent,
        seq: Serial,
    ) {
        match self {
            Self::WlSurface(w) => TouchTarget::motion(w, seat, data, event, seq),
            Self::Window(w) => {
                TouchTarget::motion(w.toplevel().unwrap().wl_surface(), seat, data, event, seq)
            }
            Self::Egui(_) => (),
        }
    }

    fn frame(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {
        match self {
            Self::WlSurface(w) => TouchTarget::frame(w, seat, data, seq),
            Self::Window(w) => {
                TouchTarget::frame(w.toplevel().unwrap().wl_surface(), seat, data, seq)
            }
            Self::Egui(_) => (),
        }
    }

    fn cancel(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {
        match self {
            Self::WlSurface(w) => TouchTarget::cancel(w, seat, data, seq),
            Self::Window(w) => {
                TouchTarget::cancel(w.toplevel().unwrap().wl_surface(), seat, data, seq)
            }
            Self::Egui(_) => (),
        }
    }

    fn shape(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::touch::ShapeEvent,
        seq: Serial,
    ) {
        match self {
            Self::WlSurface(w) => TouchTarget::shape(w, seat, data, event, seq),
            Self::Window(w) => {
                TouchTarget::shape(w.toplevel().unwrap().wl_surface(), seat, data, event, seq)
            }
            Self::Egui(_) => (),
        }
    }

    fn orientation(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &smithay::input::touch::OrientationEvent,
        seq: Serial,
    ) {
        match self {
            Self::WlSurface(w) => TouchTarget::orientation(w, seat, data, event, seq),
            Self::Window(w) => {
                TouchTarget::orientation(w.toplevel().unwrap().wl_surface(), seat, data, event, seq)
            }
            Self::Egui(_) => (),
        }
    }
}
