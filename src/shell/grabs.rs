use std::cell::RefCell;

use smithay::input::pointer::{
    AxisFrame, ButtonEvent, CursorIcon, CursorImageStatus, GestureHoldBeginEvent,
    GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
    GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
    GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
    RelativeMotionEvent,
};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge as XdgResizeEdge;
use smithay::utils::{IsAlive, Logical, Point, Rectangle, Serial, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;

use super::workspaces::WorkspaceLayout;
use super::PointerFocusTarget;
use crate::config::CONFIG;
use crate::state::State;
use crate::window::Window;

#[allow(unused)]
pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData<State>,
    pub window: Window,
    pub initial_window_geometry: Rectangle<i32, Logical>,
    last_window_location: Point<i32, Logical>,
    last_pointer_location: Point<f64, Logical>,
}

impl MoveSurfaceGrab {
    pub fn new(
        start_data: PointerGrabStartData<State>,
        window: Window,
        initial_window_geometry: Rectangle<i32, Logical>,
    ) -> Self {
        Self {
            start_data,
            window,
            initial_window_geometry,
            last_window_location: Point::default(),
            last_pointer_location: Point::default(),
        }
    }
}

#[allow(unused)]
#[allow(dead_code)]
impl PointerGrab<State> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        todo!()
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &ButtonEvent,
    ) {
        todo!()
    }

    fn axis(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &PointerGrabStartData<State> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut State) {}
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct ResizeEdge: u32 {
        const NONE = 0;
        const TOP = 1;
        const BOTTOM = 2;
        const LEFT = 4;
        const RIGHT = 8;
        // Corners
        const TOP_LEFT = Self::TOP.bits() | Self::LEFT.bits();
        const BOTTOM_LEFT = Self::BOTTOM.bits() | Self::LEFT.bits();
        const TOP_RIGHT = Self::TOP.bits() | Self::RIGHT.bits();
        const BOTTOM_RIGHT = Self::BOTTOM.bits() | Self::RIGHT.bits();
        // Sides
        const LEFT_RIGHT = Self::LEFT.bits() | Self::RIGHT.bits();
        const TOP_BOTTOM = Self::TOP.bits() | Self::BOTTOM.bits();
    }
}

#[rustfmt::skip]
impl ResizeEdge {
    pub fn cursor_icon(&self) -> CursorIcon {
        match *self {
            Self::TOP          => CursorIcon::NResize,
            Self::BOTTOM       => CursorIcon::SResize,
            Self::LEFT         => CursorIcon::WResize,
            Self::RIGHT        => CursorIcon::EResize,
            Self::TOP_LEFT     => CursorIcon::NwResize,
            Self::TOP_RIGHT    => CursorIcon::NeResize,
            Self::BOTTOM_LEFT  => CursorIcon::SwResize,
            Self::BOTTOM_RIGHT => CursorIcon::SeResize,
            _                  => CursorIcon::Default,
        }
    }
}

impl From<XdgResizeEdge> for ResizeEdge {
    #[inline]
    fn from(x: XdgResizeEdge) -> Self {
        Self::from_bits(x as u32).unwrap()
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ResizeData {
    pub edges: ResizeEdge,
    pub initial_window_location: Point<i32, Logical>,
    pub initial_window_size: Size<i32, Logical>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum ResizeState {
    #[default]
    NotResizing,
    Resizing(ResizeData),
    WaitingForFinalAck(ResizeData, Serial),
    WaitingForCommit(ResizeData),
}

pub struct PointerResizeSurfaceGrab {
    pub start_data: PointerGrabStartData<State>,
    pub window: Window,
    pub edges: ResizeEdge,
    pub initial_window_size: Size<i32, Logical>,
    // The last registered client fact.
    //
    // Used to adapt the layouts on the fly without having to store the current window size, since
    // we are not the one to calculate it
    initial_cfact: Option<f32>,
}

impl PointerResizeSurfaceGrab {
    pub fn new(
        start_data: PointerGrabStartData<State>,
        window: Window,
        edges: ResizeEdge,
        initial_window_size: Size<i32, Logical>,
    ) -> Self {
        Self {
            start_data,
            window,
            edges,
            initial_window_size,
            initial_cfact: None,
        }
    }
}

impl PointerGrab<State> for PointerResizeSurfaceGrab {
    fn motion(
        &mut self,
        state: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        todo!()
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &ButtonEvent,
    ) {
        if handle.current_pressed().is_empty() {
            // No more buttons are pressed, release the grab.
            handle.unset_grab(self, data, event.serial, event.time, true);

            // If toplevel is dead, we can't resize it, so we return early.
            if !self.window.alive() {
                return;
            }

            with_states(&self.window.wl_surface().unwrap(), |states| {
                let state = &mut *states
                    .data_map
                    .get::<RefCell<ResizeState>>()
                    .unwrap()
                    .borrow_mut();
                *state = match std::mem::take(state) {
                    ResizeState::Resizing(data) => {
                        ResizeState::WaitingForFinalAck(data, event.serial)
                    }
                    _ => unreachable!("Invalid resize state!"),
                }
            });

            handle.unset_grab(self, data, event.serial, event.time, true);
            // NOTE: Unsetting the only button to notify that we also aren't clicking. This is
            // required for user-created grabs.
            handle.button(
                data,
                &ButtonEvent {
                    state: smithay::backend::input::ButtonState::Released,
                    ..*event
                },
            );
        }
    }

    fn axis(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        details: AxisFrame,
    ) {
        handle.axis(data, details)
    }

    fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &PointerGrabStartData<State> {
        &self.start_data
    }

    fn unset(&mut self, data: &mut State) {
        // Reset the cursor to default.
        // FIXME: Maybe check if we actually changed something?
        data.fht
            .cursor_theme_manager
            .set_image_status(CursorImageStatus::default_named());
        data.fht.resize_grab_active = false;
    }
}
