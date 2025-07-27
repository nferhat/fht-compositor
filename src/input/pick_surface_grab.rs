//! A grab to pick a window/layer surface.

use async_channel::Sender;
use fht_compositor_ipc::{PickLayerShellResult, PickWindowResult};
use smithay::backend::input::ButtonState;
use smithay::desktop::{layer_map_for_output, LayerSurface, WindowSurfaceType};
use smithay::input::pointer::{ButtonEvent, GrabStartData, PointerGrab, PointerInnerHandle};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::state::State;

pub struct PickSurfaceGrab {
    pub target: PickSurfaceTarget,
    pub start_data: GrabStartData<State>,
}

pub enum PickSurfaceTarget {
    Window(Sender<PickWindowResult>),
    LayerSurface(Sender<PickLayerShellResult>),
}

impl PickSurfaceTarget {
    #[allow(clippy::type_complexity)]
    fn picked_window(&mut self, window_id: usize) {
        match self {
            PickSurfaceTarget::Window(sender) => {
                _ = sender.try_send(PickWindowResult::Some(window_id));
            }
            PickSurfaceTarget::LayerSurface(sender) => {
                _ = sender.try_send(PickLayerShellResult::None);
            }
        }
    }

    fn picked_layer_surface(&mut self, layer_surface: &LayerSurface, output: String) {
        let ipc_layer = fht_compositor_ipc::LayerShell {
            namespace: layer_surface.namespace().to_string(),
            output,
            // SAFETY: We know that all the enum variants are the same
            #[allow(clippy::missing_transmute_annotations)]
            layer: unsafe { std::mem::transmute(layer_surface.layer()) },
            #[allow(clippy::missing_transmute_annotations)]
            keyboard_interactivity: unsafe {
                std::mem::transmute(layer_surface.cached_state().keyboard_interactivity)
            },
        };

        match self {
            PickSurfaceTarget::Window(sender) => {
                _ = sender.try_send(PickWindowResult::None);
            }
            PickSurfaceTarget::LayerSurface(sender) => {
                _ = sender.try_send(PickLayerShellResult::Some(ipc_layer));
            }
        }
    }

    pub fn cancel(&mut self) {
        // NOTE: Use try_send since we might have used the sender by now, so the channel is now full
        // and the IPC client would have dropped the received by then.
        match self {
            PickSurfaceTarget::Window(sender) => {
                _ = sender.try_send(PickWindowResult::Cancelled);
            }
            PickSurfaceTarget::LayerSurface(sender) => {
                _ = sender.try_send(PickLayerShellResult::Cancelled);
            }
        }
    }
}

impl PointerGrab<State> for PickSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(
            <State as smithay::input::SeatHandler>::PointerFocus,
            smithay::utils::Point<f64, smithay::utils::Logical>,
        )>,
        event: &smithay::input::pointer::MotionEvent,
    ) {
        handle.motion(data, focus, event);
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(
            <State as smithay::input::SeatHandler>::PointerFocus,
            smithay::utils::Point<f64, smithay::utils::Logical>,
        )>,
        event: &smithay::input::pointer::RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &ButtonEvent,
    ) {
        const BTN_LEFT: u32 = 0x110;
        if event.button != BTN_LEFT || event.state != ButtonState::Pressed {
            // Pass all events as normal, we only care about left clicks.
            handle.button(data, event);
            return;
        }

        if data.fht.is_locked() {
            // Do not allow information to leak when locked
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }

        let mut tile_under = None;
        let mut layer_under = None;

        // NOTE: Keep this logic up-to-date with State::update_keyboard_focus to avoid inconsistency
        // between whats being picked and whats being focused.
        //
        // NOTE: For layer-shells we dont check for keyboard interactivity since its pointless to
        // filter based on that for picking layer-shells

        let output = &data.fht.space.active_output().clone();
        let output_loc = output.current_location();
        let pointer_loc = handle.current_location();

        let layer_map = layer_map_for_output(output);
        let monitor = data
            .fht
            .space
            .monitor_for_output(output)
            .expect("focused output should always have a monitor");

        if let Some(layer) = layer_map.layer_under(Layer::Overlay, pointer_loc) {
            let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
            if layer
                .surface_under(
                    pointer_loc - output_loc.to_f64() - layer_loc.to_f64(),
                    WindowSurfaceType::ALL,
                )
                .is_some()
            {
                layer_under = Some((layer.clone(), output.name()));
            }
        } else if let Some(fullscreen) = monitor.active_workspace().fullscreened_tile() {
            // Fullscreen focus is always exclusive
            if fullscreen
                .window()
                .surface_under(pointer_loc - output_loc.to_f64(), WindowSurfaceType::ALL)
                .is_some()
            {
                tile_under = Some(fullscreen.window().id());
            }
        } else if let Some(layer) = layer_map.layer_under(Layer::Top, pointer_loc) {
            let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
            if layer
                .surface_under(
                    pointer_loc - output_loc.to_f64() - layer_loc.to_f64(),
                    WindowSurfaceType::ALL,
                )
                .is_some()
            {
                layer_under = Some((layer.clone(), output.name()));
            }
        } else if let Some((window, _)) = data.fht.space.window_under(pointer_loc) {
            tile_under = Some(window.id());
        } else if let Some(layer) = layer_map
            .layer_under(Layer::Bottom, pointer_loc)
            .or_else(|| layer_map.layer_under(Layer::Background, pointer_loc))
        {
            let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
            if layer
                .surface_under(
                    pointer_loc - output_loc.to_f64() - layer_loc.to_f64(),
                    WindowSurfaceType::ALL,
                )
                .is_some()
            {
                layer_under = Some((layer.clone(), output.name()));
            }
        }

        assert!(
            tile_under.is_none() || layer_under.is_none(),
            "picked both a window AND a layer-shell"
        );

        if let Some(id) = tile_under {
            self.target.picked_window(*id);
        }

        if let Some((layer, output)) = layer_under {
            self.target.picked_layer_surface(&layer, output);
        }

        // And we dont make the button event go through, though we remove the grab.
        handle.unset_grab(self, data, event.serial, event.time, true);
    }

    fn axis(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        details: smithay::input::pointer::AxisFrame,
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
        event: &smithay::input::pointer::GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &GrabStartData<State> {
        &self.start_data
    }

    fn unset(&mut self, _: &mut State) {
        self.target.cancel();
    }
}
