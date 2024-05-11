use smithay::desktop::Window;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
    GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab,
    PointerInnerHandle, RelativeMotionEvent,
};
use smithay::utils::{Logical, Point};

use super::PointerFocusTarget;
use crate::state::State;
use crate::utils::geometry::{Global, PointExt, PointGlobalExt};

#[allow(unused)]
pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData<State>,
    pub window: Window,
    pub initial_window_location: Point<i32, Global>,
    pub last_location: Point<i32, Global>,
}

#[allow(unused)]
#[allow(dead_code)]
impl PointerGrab<State> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _: Option<(PointerFocusTarget, Point<i32, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the grab handle is active, no client should have focus, otherwise bad stuff WILL
        // HAPPEN (dont ask me how I know, I should probably read reference source code better)
        handle.motion(data, None, event);

        // When a user drags a workspace tile, it enters in a "free" state.
        //
        // In this state, the actual window can be dragged around, but the tile itself will still
        // be understood as its last location for the other tiles.
        //
        // At the old window location will be drawn a solid rectangle meant to represent a
        // placeholder for the old window.

        self.last_location = event.location.as_global().to_i32_round();
        let position_delta = (event.location - self.start_data.location).as_global();
        let mut new_location = self.initial_window_location.to_f64() + position_delta;
        new_location = data.clamp_coords(new_location);

        let Some(ws) = data.fht.ws_mut_for(&self.window) else { return };
        let new_location = new_location.to_local(&ws.output).to_i32_round();

        let Some(tile) = ws.tile_mut_for(&self.window) else { return };
        tile.temporary_render_location = Some(new_location);
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(PointerFocusTarget, Point<i32, Logical>)>,
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
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            // Reset the tile location.
            if let Some(ws) = data.fht.ws_mut_for(&self.window) {
                let self_tile = ws.tile_mut_for(&self.window).unwrap();
                self_tile.temporary_render_location = None;

                let mut other_window = None;
                if let Some(other_tile) = ws.tiles_under(self.last_location.to_f64()).filter(|tile| tile.element != self.window).next() {
                    other_window = Some(other_tile.element.clone());
                }

                if let Some(ref other_window) = other_window {
                    ws.swap_elements(&self.window, other_window)
                }
            }
            if let Some(tile) = data.fht.tile_mut_for(&self.window) {
                tile.temporary_render_location = None;
            }


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
