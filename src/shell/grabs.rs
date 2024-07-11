use std::cell::RefCell;

use smithay::desktop::Window;
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
use crate::utils::geometry::{Global, Local, PointExt, PointGlobalExt, SizeExt};

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

/// Information about the resize operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ResizeData {
    /// The edges the surface is being resized with.
    pub edges: ResizeEdge,
    /// The initial window location.
    pub initial_window_location: Point<i32, Logical>,
    /// The initial window size (geometry width and height).
    pub initial_window_size: Size<i32, Logical>,
}

/// State of the resize operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum ResizeState {
    /// The surface is not being resized.
    #[default]
    NotResizing,
    /// The surface is currently being resized.
    Resizing(ResizeData),
    /// The resize has finished, and the surface needs to ack the final configure.
    WaitingForFinalAck(ResizeData, Serial),
    /// The resize has finished, and the surface needs to commit its final state.
    WaitingForCommit(ResizeData),
}

pub struct PointerResizeSurfaceGrab {
    pub start_data: PointerGrabStartData<State>,
    pub window: Window,
    pub edges: ResizeEdge,
    pub initial_window_size: Size<i32, Local>,
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
        initial_window_size: Size<i32, Local>,
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
        // While the grab is active, no client has pointer focus
        handle.motion(state, None, event);

        if !self.window.alive() {
            handle.unset_grab(self, state, event.serial, event.time, true);
            return;
        }

        let mut delta: Point<i32, Logical> =
            (event.location - self.start_data.location).to_i32_round();
        let mut new_size = self.initial_window_size.as_logical();

        // Custom behaviour for tiled layouts.
        //
        // Since we have a cfact and mwfact system going on, based on how you resize the window,
        // the layout will adapt accordingly to reflect the new window size, while also moving and
        // changing all the other windows with it.
        //
        // Based on the layout, the new width/height delta will change and adapt the mwfact and
        // cfacts of the the workspace layout.
        //
        // Depending on how and where our windows are, we should only allow them to grow in a
        // certain direction. the Read a bit more below about how layouts should react when doing
        // interactive window resizes.
        //
        // Taking into account all these considerations and quirks provide a smooth interactive
        // resize experience, at the cost of handling a good chunk of edge cases that doesnt.
        //
        // The main challenge here is to make windows grow or shrink their column/stack
        // (master and slave) in the same direction no matter what side we are grabbing them from.
        // Depending on the layout, we need to invert their the X delta or the Y delta to achieve
        // this.

        let ws = state.fht.ws_mut_for(&self.window).unwrap();
        let windows_len = ws.tiles.len();

        // First of all: no need todo anything if we only have one window.
        if windows_len == 1 {
            return;
        }

        let window_idx = ws
            .tiles
            .iter()
            .position(|tile| tile.element() == &self.window)
            .unwrap();
        let tile_area = ws.tile_area();
        // Try to update only if it matters.
        let mut new_mwfact @ mut cfact_delta = Option::<f32>::None;

        match ws.get_active_layout() {
            WorkspaceLayout::Tile { nmaster, .. } => {
                let is_master = window_idx < nmaster;
                let is_on_top_side = window_idx == 0 || window_idx == nmaster;
                let is_on_bottom_side =
                    window_idx == nmaster.saturating_sub(1) || window_idx == windows_len - 1;
                let is_in_middle = !is_on_top_side && !is_on_bottom_side;

                if self.edges.intersects(ResizeEdge::LEFT_RIGHT) {
                    // For this layout, the master stack does not need to be changed.
                    //
                    // However, if we are the slave stack, our delta gets automatically negated
                    // since we calculate the mwfact from it using `1.0 - mwfact` since we take
                    // what the master client doesn't.
                    // So to match the growing/shrinking of the slave stack appropriatly, we negate
                    // it.
                    if !is_master {
                        delta.x = -delta.x;
                    }

                    new_size.w += delta.x;

                    new_mwfact = Some(if is_master {
                        new_size.w as f32 / (tile_area.size.w - CONFIG.general.inner_gaps) as f32
                    } else {
                        1.0 - (new_size.w as f32
                            / (tile_area.size.w - CONFIG.general.inner_gaps) as f32)
                    });
                }

                if self.edges.intersects(ResizeEdge::TOP_BOTTOM) {
                    // The reason of why we need to invert the sign here is because of two factors:
                    // - How points and geometry are done in smithay
                    // - How we calculate the delta itself.
                    //
                    // Basically, rectangles and geometry are defined using the top-left corner of
                    // a rectangle, this is important since when we caculate the delta, going up
                    // means the mouse position will be smaller, resulting in a negative delta the
                    // more you go up.
                    //
                    // But, in both the following cases, the user excepts the mouse to grow the
                    // window, but if we don't invert the delta, it will actually (due to math)
                    // make it smaller.
                    if is_on_bottom_side || (is_in_middle && self.edges.intersects(ResizeEdge::TOP))
                    {
                        delta.y = -delta.y;
                    }
                    new_size.h += delta.y;

                    cfact_delta = Some(new_size.h as f32 / self.initial_window_size.h as f32);
                }
            }
            WorkspaceLayout::BottomStack { nmaster, .. } => {
                let is_master = window_idx < nmaster;
                let is_on_left_side = window_idx == 0 || window_idx == nmaster;
                let is_on_right_side =
                    window_idx == nmaster.saturating_sub(1) || window_idx == windows_len - 1;
                let is_in_middle = !is_on_left_side && !is_on_right_side;

                if self.edges.intersects(ResizeEdge::LEFT_RIGHT) {
                    // This is the same logic with the TOP_BOTTOM check with the Tile layout.
                    // Go read it, but instead of being at the bottom this time, we are at the
                    // right.
                    if is_on_right_side || (is_in_middle && self.edges.intersects(ResizeEdge::LEFT))
                    {
                        delta.x = -delta.x;
                    }
                    new_size.w += delta.x;

                    cfact_delta = Some(new_size.w as f32 / self.initial_window_size.w as f32);
                }

                if self.edges.intersects(ResizeEdge::TOP_BOTTOM) {
                    // Same reason as LEFT_RIGHT section of the Tile layout, but here the mwfact
                    // determines the height of the master and slave stack, not the width.
                    if !is_master {
                        delta.y = -delta.y;
                    }
                    new_size.h += delta.y;

                    new_mwfact = Some(if is_master {
                        new_size.h as f32 / (tile_area.size.h - CONFIG.general.inner_gaps) as f32
                    } else {
                        1.0 - (new_size.h as f32
                            / (tile_area.size.h - CONFIG.general.inner_gaps) as f32)
                    });
                }
            }
            WorkspaceLayout::CenteredMaster { nmaster, .. } => {
                // For the centered master layout, its a little more complicated.
                //
                // The master stack grows to both directions at the same time.
                //
                // For the other two columns: the left column grows to the right, the right columns
                // grows to the left. You are on the left if the stack_idx (so the index of the
                // window relative to the beginning of the stack, aka window_idx - nmaster) % 2 ==
                // 0. You are on the left if != 0
                //
                // To know if the window is on the topside or bottom side, its not that convoluted,
                // its always by how the layout works the two first stack window or the two last
                // ones, with ofc the master column.
                let is_master = window_idx < nmaster;
                let stack_idx = window_idx.saturating_sub(nmaster);
                let is_right_column = stack_idx % 2 == 0;
                let is_on_top_side = window_idx == 0 || stack_idx <= 1;
                let is_on_bottom_side = window_idx == nmaster.saturating_sub(1)
                    || windows_len.saturating_sub(window_idx + 1) <= 1;
                let is_in_middle = !is_on_top_side || !is_on_bottom_side;

                if self.edges.intersects(ResizeEdge::LEFT_RIGHT) {
                    // Both of these are inverted for the same reason as the LEFT_RIGHTcheck with
                    // the bottom stack layout.
                    if (is_master && self.edges.intersects(ResizeEdge::LEFT))
                        || (!is_master && is_right_column)
                    {
                        delta.x = -delta.x;
                    }

                    new_size.w += delta.x;

                    // Centered master can have one or TWO columns, important to note this
                    if is_master || ws.tiles.len() < 3 {
                        // Though, if we are the master, we dont care whether we are one or two
                        // columns, since we determine the mwfact ourselves
                        new_mwfact = Some(
                            new_size.w as f32
                                / (tile_area.size.w - CONFIG.general.inner_gaps) as f32,
                        );
                    } else {
                        // But, if we are the clients, we have two multiply by two only if we have
                        // two columns, This is the case when we have more
                        // than three windows in the layout.
                        new_mwfact = Some(
                            1.0 - new_size.w as f32
                                / (tile_area.size.w - CONFIG.general.inner_gaps) as f32
                                * 2.0,
                        );
                    }
                }

                if self.edges.intersects(ResizeEdge::TOP_BOTTOM) {
                    // This is the same logic with the TOP_BOTTOM check with the Tile layout.
                    if is_on_bottom_side && self.edges.intersects(ResizeEdge::BOTTOM) {
                        delta.y = -delta.y;
                    }
                    if is_in_middle && self.edges.intersects(ResizeEdge::BOTTOM) {
                        delta.y = -delta.y;
                    }

                    new_size.h += delta.y;
                    cfact_delta = Some(self.initial_window_size.h as f32 / new_size.h as f32);
                }
            }
            WorkspaceLayout::Floating => {}
        };

        let mut arrange = false;
        if let Some(new_mwfact) = new_mwfact {
            let active_layout = &mut ws.layouts[ws.active_layout_idx];
            if let WorkspaceLayout::Tile {
                master_width_factor,
                ..
            }
            | WorkspaceLayout::BottomStack {
                master_width_factor,
                ..
            }
            | WorkspaceLayout::CenteredMaster {
                master_width_factor,
                ..
            } = active_layout
            {
                *master_width_factor = new_mwfact;
                *master_width_factor = master_width_factor.clamp(0.05, 0.95);
            }
            arrange = true;
        }
        if let Some(delta) = cfact_delta {
            let tile = ws.tile_mut_for(&self.window).unwrap();
            let initial_cfact = *self.initial_cfact.get_or_insert(tile.cfact);
            // NOTE: -1.0 since the delta starts at 1.0, since its new_size/old_size
            tile.cfact = (initial_cfact + delta - 1.0).clamp(0.5, 10.);
            arrange = true;
        }

        if arrange {
            ws.arrange_tiles();
        }
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
        let mut lock = data.fht.cursor_theme_manager.image_status.lock().unwrap();
        *lock = CursorImageStatus::default_named();
    }
}
