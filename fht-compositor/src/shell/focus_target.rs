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
pub use smithay::input::Seat;
pub use smithay::reexports::wayland_server::backend::ObjectId;
pub use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
pub use smithay::reexports::wayland_server::Resource;
pub use smithay::utils::{IsAlive, Serial};
pub use smithay::wayland::seat::WaylandFocus;

use crate::shell::FhtWindow;
use crate::state::State;

#[derive(Debug, Clone, PartialEq)]
pub enum FocusTarget {
    Window(FhtWindow),
    LayerSurface(LayerSurface),
    Popup(PopupKind),
}

impl IsAlive for FocusTarget {
    fn alive(&self) -> bool {
        match self {
            FocusTarget::Window(w) => w.alive(),
            FocusTarget::LayerSurface(l) => l.alive(),
            FocusTarget::Popup(p) => p.alive(),
        }
    }
}

impl From<FocusTarget> for WlSurface {
    fn from(target: FocusTarget) -> Self {
        target.wl_surface().unwrap()
    }
}

impl PointerTarget<State> for FocusTarget {
    fn enter(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {
        match self {
            FocusTarget::Window(w) => PointerTarget::enter(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::enter(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::enter(p.wl_surface(), seat, data, event),
        }
    }
    fn motion(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {
        match self {
            FocusTarget::Window(w) => PointerTarget::motion(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::motion(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::motion(p.wl_surface(), seat, data, event),
        }
    }
    fn relative_motion(&self, seat: &Seat<State>, data: &mut State, event: &RelativeMotionEvent) {
        match self {
            FocusTarget::Window(w) => PointerTarget::relative_motion(w, seat, data, event),
            FocusTarget::LayerSurface(l) => {
                PointerTarget::relative_motion(l.wl_surface(), seat, data, event)
            }
            FocusTarget::Popup(p) => {
                PointerTarget::relative_motion(p.wl_surface(), seat, data, event)
            }
        }
    }
    fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {
        match self {
            FocusTarget::Window(w) => PointerTarget::button(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::button(l, seat, data, event),
            FocusTarget::Popup(p) => PointerTarget::button(p.wl_surface(), seat, data, event),
        }
    }
    fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {
        match self {
            FocusTarget::Window(w) => PointerTarget::axis(w, seat, data, frame),
            FocusTarget::LayerSurface(l) => PointerTarget::axis(l, seat, data, frame),
            FocusTarget::Popup(p) => PointerTarget::axis(p.wl_surface(), seat, data, frame),
        }
    }
    fn frame(&self, seat: &Seat<State>, data: &mut State) {
        match self {
            FocusTarget::Window(w) => PointerTarget::frame(w, seat, data),
            FocusTarget::LayerSurface(l) => PointerTarget::frame(l, seat, data),
            FocusTarget::Popup(p) => PointerTarget::frame(p.wl_surface(), seat, data),
        }
    }
    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {
        match self {
            FocusTarget::Window(w) => PointerTarget::leave(w, seat, data, serial, time),
            FocusTarget::LayerSurface(l) => PointerTarget::leave(l, seat, data, serial, time),
            FocusTarget::Popup(p) => PointerTarget::leave(p.wl_surface(), seat, data, serial, time),
        }
    }
    fn gesture_swipe_begin(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        event: &GestureSwipeBeginEvent,
    ) {
        match self {
            FocusTarget::Window(w) => PointerTarget::gesture_swipe_begin(w, seat, data, event),
            FocusTarget::LayerSurface(l) => {
                PointerTarget::gesture_swipe_begin(l, seat, data, event)
            }
            FocusTarget::Popup(p) => {
                PointerTarget::gesture_swipe_begin(p.wl_surface(), seat, data, event)
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
            FocusTarget::Window(w) => PointerTarget::gesture_swipe_update(w, seat, data, event),
            FocusTarget::LayerSurface(l) => {
                PointerTarget::gesture_swipe_update(l, seat, data, event)
            }
            FocusTarget::Popup(p) => {
                PointerTarget::gesture_swipe_update(p.wl_surface(), seat, data, event)
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
            FocusTarget::Window(w) => PointerTarget::gesture_swipe_end(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::gesture_swipe_end(l, seat, data, event),
            FocusTarget::Popup(p) => {
                PointerTarget::gesture_swipe_end(p.wl_surface(), seat, data, event)
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
            FocusTarget::Window(w) => PointerTarget::gesture_pinch_begin(w, seat, data, event),
            FocusTarget::LayerSurface(l) => {
                PointerTarget::gesture_pinch_begin(l, seat, data, event)
            }
            FocusTarget::Popup(p) => {
                PointerTarget::gesture_pinch_begin(p.wl_surface(), seat, data, event)
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
            FocusTarget::Window(w) => PointerTarget::gesture_pinch_update(w, seat, data, event),
            FocusTarget::LayerSurface(l) => {
                PointerTarget::gesture_pinch_update(l, seat, data, event)
            }
            FocusTarget::Popup(p) => {
                PointerTarget::gesture_pinch_update(p.wl_surface(), seat, data, event)
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
            FocusTarget::Window(w) => PointerTarget::gesture_pinch_end(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::gesture_pinch_end(l, seat, data, event),
            FocusTarget::Popup(p) => {
                PointerTarget::gesture_pinch_end(p.wl_surface(), seat, data, event)
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
            FocusTarget::Window(w) => PointerTarget::gesture_hold_begin(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::gesture_hold_begin(l, seat, data, event),
            FocusTarget::Popup(p) => {
                PointerTarget::gesture_hold_begin(p.wl_surface(), seat, data, event)
            }
        }
    }
    fn gesture_hold_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldEndEvent) {
        match self {
            FocusTarget::Window(w) => PointerTarget::gesture_hold_end(w, seat, data, event),
            FocusTarget::LayerSurface(l) => PointerTarget::gesture_hold_end(l, seat, data, event),
            FocusTarget::Popup(p) => {
                PointerTarget::gesture_hold_end(p.wl_surface(), seat, data, event)
            }
        }
    }
}

impl KeyboardTarget<State> for FocusTarget {
    fn enter(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            FocusTarget::Window(w) => KeyboardTarget::enter(w, seat, data, keys, serial),
            FocusTarget::LayerSurface(l) => KeyboardTarget::enter(l, seat, data, keys, serial),
            FocusTarget::Popup(p) => {
                KeyboardTarget::enter(p.wl_surface(), seat, data, keys, serial)
            }
        }
    }
    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {
        match self {
            FocusTarget::Window(w) => KeyboardTarget::leave(w, seat, data, serial),
            FocusTarget::LayerSurface(l) => KeyboardTarget::leave(l, seat, data, serial),
            FocusTarget::Popup(p) => KeyboardTarget::leave(p.wl_surface(), seat, data, serial),
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
            FocusTarget::Window(w) => KeyboardTarget::key(w, seat, data, key, state, serial, time),
            FocusTarget::LayerSurface(l) => {
                KeyboardTarget::key(l, seat, data, key, state, serial, time)
            }
            FocusTarget::Popup(p) => {
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
            FocusTarget::Window(w) => KeyboardTarget::modifiers(w, seat, data, modifiers, serial),
            FocusTarget::LayerSurface(l) => {
                KeyboardTarget::modifiers(l, seat, data, modifiers, serial)
            }
            FocusTarget::Popup(p) => {
                KeyboardTarget::modifiers(p.wl_surface(), seat, data, modifiers, serial)
            }
        }
    }
}

impl WaylandFocus for FocusTarget {
    fn wl_surface(&self) -> Option<WlSurface> {
        match self {
            FocusTarget::Window(w) => w.wl_surface(),
            FocusTarget::LayerSurface(l) => Some(l.wl_surface().clone()),
            FocusTarget::Popup(p) => Some(p.wl_surface().clone()),
        }
    }
    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            FocusTarget::Window(w) => w.same_client_as(object_id),
            FocusTarget::LayerSurface(l) => l.wl_surface().id().same_client_as(object_id),
            FocusTarget::Popup(p) => p.wl_surface().id().same_client_as(object_id),
        }
    }
}

impl From<FhtWindow> for FocusTarget {
    fn from(w: FhtWindow) -> Self {
        FocusTarget::Window(w)
    }
}

impl From<LayerSurface> for FocusTarget {
    fn from(l: LayerSurface) -> Self {
        FocusTarget::LayerSurface(l)
    }
}

impl From<PopupKind> for FocusTarget {
    fn from(p: PopupKind) -> Self {
        FocusTarget::Popup(p)
    }
}
