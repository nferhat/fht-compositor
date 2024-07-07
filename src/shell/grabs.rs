use smithay::desktop::Window;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
    GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab,
    PointerInnerHandle, RelativeMotionEvent,
};
use smithay::utils::{Logical, Point, Rectangle};

use super::PointerFocusTarget;
use crate::state::State;
use crate::utils::geometry::{Global, Local, PointExt, PointGlobalExt};

#[allow(unused)]
pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData<State>,
    /// The concerned window.
    pub window: Window,
    /// The initial window geometry we started with before dragging the tile.
    pub initial_window_geometry: Rectangle<i32, Global>,
    /// The last registered window location.
    last_window_location: Point<i32, Local>,
    /// The last registered pointer location.
    ///
    /// Keeping at as f64, Global marker since workspaces transform them locally automatically.
    last_pointer_location: Point<f64, Global>,
}

impl MoveSurfaceGrab {
    pub fn new(
        start_data: PointerGrabStartData<State>,
        window: Window,
        initial_window_geometry: Rectangle<i32, Global>,
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

        let position_delta = (event.location - self.start_data.location).as_global();
        let mut new_location = self.initial_window_geometry.loc.to_f64() + position_delta;
        new_location = data.clamp_coords(new_location);

        let Some(ws) = data.fht.ws_mut_for(&self.window) else {
            return;
        };
        let new_location = new_location.to_local(&ws.output).to_i32_round();

        self.last_pointer_location = event.location.as_global();
        self.last_window_location = new_location;

        let Some(tile) = ws.tile_mut_for(&self.window) else {
            // Window dead/moved, no need to persist the grab
            handle.unset_grab(self, data, event.serial, event.time, true);
            // NOTE: Unsetting the only button to notify that we also aren't clicking. This is
            // required for user-created grabs.
            handle.button(
                data,
                &ButtonEvent {
                    state: smithay::backend::input::ButtonState::Released,
                    button: 0x110, // left mouse button
                    time: event.time,
                    serial: event.serial,
                },
            );
            return;
        };
        tile.temporary_render_location = Some(new_location);
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
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            if let Some(ws) = data.fht.ws_mut_for(&self.window) {
                // When we drop the tile, instead of animating from its old location to the new
                // one, we want for it to continue from our dragged position.
                //
                // So, we set our tile location so that when we swap out the two, the arrange_tiles
                // function (used in swap_elements) will animate from this location here.
                let location = self.last_pointer_location.to_local(&ws.output);
                let self_tile = ws.tile_mut_for(&self.window).unwrap();
                self_tile.temporary_render_location = None;
                self_tile.location = self.last_window_location;

                // Though we only want to update our location when we actuall are goin to swap
                let other_window = ws
                    .tiles_under(self.last_pointer_location.to_f64())
                    .find(|tile| tile.element != self.window)
                    .map(|tile| tile.element.clone());

                if let Some(ref other_window) = other_window {
                    ws.swap_elements(&self.window, other_window)
                } else {
                    // If we didnt find anything to swap with, still animate the window back.
                    ws.arrange_tiles();
                }
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
