use std::borrow::Cow;

pub use smithay::backend::input::KeyState;
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
use smithay::wayland::session_lock::LockSurface;

use crate::state::State;
use crate::window::Window;

#[derive(Clone, Debug, PartialEq)]
pub enum KeyboardFocusTarget {
    Window(Window),
    LayerSurface(LayerSurface),
    LockSurface(LockSurface),
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

impl From<LockSurface> for KeyboardFocusTarget {
    fn from(value: LockSurface) -> Self {
        Self::LockSurface(value)
    }
}

impl From<PopupKind> for KeyboardFocusTarget {
    fn from(value: PopupKind) -> Self {
        Self::Popup(value)
    }
}

impl WaylandFocus for KeyboardFocusTarget {
    fn wl_surface(&self) -> Option<Cow<WlSurface>> {
        match self {
            Self::Window(w) => w.wl_surface(),
            Self::LayerSurface(l) => Some(Cow::Owned(l.wl_surface().clone())),
            Self::LockSurface(l) => Some(Cow::Owned(l.wl_surface().clone())),
            Self::Popup(p) => Some(Cow::Owned(p.wl_surface().clone())),
        }
    }

    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            Self::Window(w) => w.same_client_as(object_id),
            Self::LayerSurface(l) => l.same_client_as(object_id),
            Self::LockSurface(l) => l.wl_surface().same_client_as(object_id),
            Self::Popup(p) => p.wl_surface().same_client_as(object_id),
        }
    }
}

impl IsAlive for KeyboardFocusTarget {
    fn alive(&self) -> bool {
        match self {
            Self::Window(w) => w.alive(),
            Self::LayerSurface(l) => l.alive(),
            Self::LockSurface(l) => l.alive(),
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
                KeyboardTarget::enter(w.toplevel().wl_surface(), seat, data, keys, serial)
            }
            Self::LayerSurface(l) => {
                KeyboardTarget::enter(l.wl_surface(), seat, data, keys, serial)
            }
            Self::LockSurface(l) => KeyboardTarget::enter(l.wl_surface(), seat, data, keys, serial),
            Self::Popup(p) => KeyboardTarget::enter(p.wl_surface(), seat, data, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {
        match self {
            Self::Window(w) => KeyboardTarget::leave(w.toplevel().wl_surface(), seat, data, serial),
            Self::LayerSurface(l) => KeyboardTarget::leave(l.wl_surface(), seat, data, serial),
            Self::LockSurface(l) => KeyboardTarget::leave(l.wl_surface(), seat, data, serial),
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
                w.toplevel().wl_surface(),
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
            Self::LockSurface(l) => {
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
            Self::Window(w) => {
                KeyboardTarget::modifiers(w.toplevel().wl_surface(), seat, data, modifiers, serial)
            }
            Self::LayerSurface(l) => {
                KeyboardTarget::modifiers(l.wl_surface(), seat, data, modifiers, serial)
            }
            Self::LockSurface(l) => {
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
}

impl From<KeyboardFocusTarget> for PointerFocusTarget {
    fn from(value: KeyboardFocusTarget) -> Self {
        match value {
            KeyboardFocusTarget::Window(w) => Self::Window(w),
            KeyboardFocusTarget::LayerSurface(surface) => {
                PointerFocusTarget::from(surface.wl_surface().clone())
            }
            KeyboardFocusTarget::LockSurface(surface) => {
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

impl From<Window> for PointerFocusTarget {
    fn from(value: Window) -> Self {
        Self::Window(value)
    }
}

impl WaylandFocus for PointerFocusTarget {
    fn wl_surface(&self) -> Option<Cow<WlSurface>> {
        match self {
            Self::WlSurface(w) => w.wl_surface(),
            Self::Window(w) => w.wl_surface(),
        }
    }
    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            Self::WlSurface(w) => w.same_client_as(object_id),
            Self::Window(w) => w.same_client_as(object_id),
        }
    }
}

impl IsAlive for PointerFocusTarget {
    fn alive(&self) -> bool {
        match self {
            Self::WlSurface(w) => w.alive(),
            Self::Window(w) => w.alive(),
        }
    }
}

impl PointerTarget<State> for PointerFocusTarget {
    fn enter(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {
        match self {
            Self::WlSurface(w) => PointerTarget::enter(w, seat, data, event),
            Self::Window(w) => PointerTarget::enter(w.toplevel().wl_surface(), seat, data, event),
        }
    }

    fn motion(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {
        match self {
            Self::WlSurface(w) => PointerTarget::motion(w, seat, data, event),
            Self::Window(w) => PointerTarget::motion(w.toplevel().wl_surface(), seat, data, event),
        }
    }

    fn relative_motion(&self, seat: &Seat<State>, data: &mut State, event: &RelativeMotionEvent) {
        match self {
            Self::WlSurface(w) => PointerTarget::relative_motion(w, seat, data, event),
            Self::Window(w) => {
                PointerTarget::relative_motion(w.toplevel().wl_surface(), seat, data, event)
            }
        }
    }

    fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {
        match self {
            Self::WlSurface(w) => PointerTarget::button(w, seat, data, event),
            Self::Window(w) => PointerTarget::button(w.toplevel().wl_surface(), seat, data, event),
        }
    }

    fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {
        match self {
            Self::WlSurface(w) => PointerTarget::axis(w, seat, data, frame),
            Self::Window(w) => PointerTarget::axis(w.toplevel().wl_surface(), seat, data, frame),
        }
    }

    fn frame(&self, seat: &Seat<State>, data: &mut State) {
        match self {
            Self::WlSurface(w) => PointerTarget::frame(w, seat, data),
            Self::Window(w) => PointerTarget::frame(w.toplevel().wl_surface(), seat, data),
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
            Self::Window(w) => {
                PointerTarget::gesture_swipe_begin(w.toplevel().wl_surface(), seat, data, event)
            }
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
            Self::Window(w) => {
                PointerTarget::gesture_swipe_update(w.toplevel().wl_surface(), seat, data, event)
            }
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
            Self::Window(w) => {
                PointerTarget::gesture_swipe_end(w.toplevel().wl_surface(), seat, data, event)
            }
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
            Self::Window(w) => {
                PointerTarget::gesture_pinch_begin(w.toplevel().wl_surface(), seat, data, event)
            }
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
            Self::Window(w) => {
                PointerTarget::gesture_pinch_update(w.toplevel().wl_surface(), seat, data, event)
            }
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
            Self::Window(w) => {
                PointerTarget::gesture_pinch_end(w.toplevel().wl_surface(), seat, data, event)
            }
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
            Self::Window(w) => {
                PointerTarget::gesture_hold_begin(w.toplevel().wl_surface(), seat, data, event)
            }
        }
    }

    fn gesture_hold_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldEndEvent) {
        match self {
            Self::WlSurface(w) => PointerTarget::gesture_hold_end(w, seat, data, event),
            Self::Window(w) => {
                PointerTarget::gesture_hold_end(w.toplevel().wl_surface(), seat, data, event)
            }
        }
    }

    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {
        match self {
            Self::WlSurface(w) => PointerTarget::leave(w, seat, data, serial, time),
            Self::Window(w) => {
                PointerTarget::leave(w.toplevel().wl_surface(), seat, data, serial, time)
            }
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
            Self::Window(w) => TouchTarget::down(w.toplevel().wl_surface(), seat, data, event, seq),
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
            Self::Window(w) => TouchTarget::up(w.toplevel().wl_surface(), seat, data, event, seq),
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
                TouchTarget::motion(w.toplevel().wl_surface(), seat, data, event, seq)
            }
        }
    }

    fn frame(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {
        match self {
            Self::WlSurface(w) => TouchTarget::frame(w, seat, data, seq),
            Self::Window(w) => TouchTarget::frame(w.toplevel().wl_surface(), seat, data, seq),
        }
    }

    fn cancel(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {
        match self {
            Self::WlSurface(w) => TouchTarget::cancel(w, seat, data, seq),
            Self::Window(w) => TouchTarget::cancel(w.toplevel().wl_surface(), seat, data, seq),
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
                TouchTarget::shape(w.toplevel().wl_surface(), seat, data, event, seq)
            }
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
                TouchTarget::orientation(w.toplevel().wl_surface(), seat, data, event, seq)
            }
        }
    }
}
