pub mod actions;
pub mod pick_surface_grab;
pub mod resize_tile_grab;
pub mod swap_tile_grab;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

pub use actions::*;
use fht_compositor_config::{GestureAction, GestureDirection, KeyPattern};
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Device, DeviceCapability, Event,
    GestureBeginEvent, GestureEndEvent, GesturePinchUpdateEvent, GestureSwipeUpdateEvent,
    InputBackend, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    PointerMotionEvent, ProximityState, TabletToolButtonEvent, TabletToolEvent,
    TabletToolProximityEvent, TabletToolTipEvent, TabletToolTipState,
};
use smithay::desktop::layer_map_for_output;
use smithay::input::keyboard::{FilterResult, Keysym, ModifiersState};
use smithay::input::pointer::{self, AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent};
use smithay::reexports::wayland_server::protocol::wl_pointer;
use smithay::utils::{IsAlive, Logical, Point, SERIAL_COUNTER};
use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat;
use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraint};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer};
use smithay::wayland::tablet_manager::{TabletDescriptor, TabletSeatTrait};

use crate::backend::Backend;
use crate::focus_target::{KeyboardFocusTarget, PointerFocusTarget};
use crate::output::OutputExt;
use crate::state::State;
use crate::utils::get_monotonic_time;

impl State {
    pub fn update_keyboard_focus(&mut self) {
        crate::profile_function!();
        let keyboard = self.fht.keyboard.clone();
        let output = self.fht.space.active_output().clone();

        // Before updating keyboard focus, make sure the layer-shell the user requested to focus
        // (by clicking) can still accept keyboard focus
        _ = self
            .fht
            .focused_on_demand_layer_shell
            .take_if(|layer_shell| {
                if !layer_shell.alive() {
                    return false; // dead, byebye
                }

                let keyboard_interactivity = layer_shell.cached_state().keyboard_interactivity;
                !matches!(
                    keyboard_interactivity,
                    KeyboardInteractivity::Exclusive | KeyboardInteractivity::OnDemand
                )
            });

        let new_focus = if self.fht.is_locked() {
            let output_state = self.fht.output_state.get(&output).unwrap();
            if let Some(lock_surface) = output_state.lock_surface.clone() {
                Some(KeyboardFocusTarget::LockSurface(lock_surface))
            } else {
                // Even if the compositor isn't locked we force remove the focus from everything
                // else here, since we might in a state when the lock program didn't assign surfaces
                // yet
                None
            }
        } else {
            let mon = self.fht.space.monitor_for_output(&output).unwrap();

            // When checking for window focus, the fullscreened window always take precedence,
            // since its the only one displayed.
            let focused_window = || {
                mon.active_workspace()
                    .active_window()
                    .map(KeyboardFocusTarget::Window)
            };
            let fullscreen_window_on_monitor = || {
                mon.active_workspace()
                    .fullscreened_window()
                    .map(KeyboardFocusTarget::Window)
            };

            // When checking for layer shell focus, exclusive keyboard focus obviously takes the
            // precedence, then we check on-demand.
            //
            // On-demand layer-shells get keyboard focus only when they get pressed down.
            let layer_map = layer_map_for_output(&output);
            let on_demand_layer_shell = |layer| {
                layer_map
                    .layers_on(layer)
                    .find(|&layer| Some(layer) == self.fht.focused_on_demand_layer_shell.as_ref())
                    .cloned()
                    .map(KeyboardFocusTarget::LayerSurface)
            };
            let exclusive_layer_shell = |layer| {
                layer_map
                    .layers_on(layer)
                    .find(|&layer| {
                        layer.cached_state().keyboard_interactivity
                            == KeyboardInteractivity::Exclusive
                    })
                    .cloned()
                    .map(KeyboardFocusTarget::LayerSurface)
            };
            let layer_shell_focus =
                |layer| exclusive_layer_shell(layer).or_else(|| on_demand_layer_shell(layer));

            // Now start checking for focus, from Overlay layer shells
            //
            // Make sure that these are ordered the same way in Fht::output_elements to ensure
            // consistency.
            let mut ft = layer_shell_focus(Layer::Overlay);
            if mon.render_above_top() {
                ft = ft.or_else(|| fullscreen_window_on_monitor());
                ft = ft.or_else(|| focused_window());
                ft = ft.or_else(|| layer_shell_focus(Layer::Top));
                ft = ft.or_else(|| layer_shell_focus(Layer::Bottom));
                ft = ft.or_else(|| layer_shell_focus(Layer::Background));
            } else {
                ft = ft.or_else(|| layer_shell_focus(Layer::Top));
                ft = ft.or_else(|| fullscreen_window_on_monitor());
                ft = ft.or_else(|| focused_window());
                ft = ft.or_else(|| layer_shell_focus(Layer::Bottom));
                ft = ft.or_else(|| layer_shell_focus(Layer::Background));
            }

            ft
        };

        if keyboard.current_focus() != new_focus {
            // Inform the workspace system about the new focus, this will in turn set the Activated
            // xdg_toplevel state on the window (after State::dispatch)
            if let Some(KeyboardFocusTarget::Window(window)) = &new_focus {
                if !self.fht.space.activate_window(window, true) {
                    // Don't really know when this can hapen
                    error!("Window from space disappeared while being focused");
                    return;
                }
            }

            // FIXME: We are not handling popup grabs here, might mess things here.
            //
            // By default anvil early returns on this function if the keyboard/pointer are grabbed,
            // but seems like a hack more like anything else
            self.set_keyboard_focus(new_focus);
        }
    }

    /// Refresh the pointer focus.
    pub fn update_pointer_focus(&mut self) {
        crate::profile_scope!("refresh_pointer_focus");
        // We try to update the pointer focus. If the new one is not the same as the previous one we
        // encountered, we send a motion and frame event to the new one.
        let pointer = self.fht.pointer.clone();
        let pointer_loc = pointer.current_location();
        let new_focus = self.fht.focus_target_under(pointer_loc);
        if new_focus.as_ref().map(|(ft, _)| ft) == pointer.current_focus().as_ref() {
            return; // No updates, keep going
        }

        pointer.motion(
            self,
            new_focus,
            &MotionEvent {
                location: pointer_loc,
                serial: SERIAL_COUNTER.next_serial(),
                time: get_monotonic_time().as_millis() as u32,
            },
        );
        // After motion, try to activate new pointer constraint under surface
        self.fht.activate_pointer_constraint();

        pointer.frame(self);
    }

    fn handle_focus_follows_mouse(&mut self, under: Option<PointerFocusTarget>) {
        // When we are handling focus-follows-mouse, we must do a few things:
        // - If we are about to focus a LayerShell and that layer-shell has OnDemand keyboard
        //   interactivity, we do the same handling as if the user clicked that layer-shell, and
        //   give keyboard focus
        // - If we are about to focus a Window, we make sure that that window is actually focused in
        //   the Space, by updating Workspace::active_tile_idx for correctness.
        let Some(new_focus) = under else { return };
        // Space updates should have been done by now.
        let output = self.fht.space.active_output();

        match new_focus {
            PointerFocusTarget::WlSurface(wl_surface) => {
                // There are two cases for this:
                // - This could be a LockSurface, which we don't care about since if there's one,
                //   its always focused by default.
                // - This can also be a window/layer-shell subsurface/popup. We traserve up the
                //   surface tree until we find a matching layer-shell/window. Root surfaces are
                //   cached on each surface commit.
                let Some(root_surface) = self.fht.root_surfaces.get(&wl_surface).cloned() else {
                    // Root surfaces are cached on initial commit and after, if we can't get it
                    // now, wait for the first commit then handle
                    return;
                };

                if let Some(window) = self.fht.space.find_window(&root_surface) {
                    _ = self.fht.space.activate_window(&window, true);
                    self.set_keyboard_focus(Some(window));
                    return;
                }

                // Try to handle for layer-shell
                let mut focus_layer = None;
                let layer_map = layer_map_for_output(output);
                if let Some(layer) = layer_map.layers().find(|layer_surface| {
                    *layer_surface.wl_surface() == root_surface
                        && matches!(layer_surface.layer(), Layer::Top | Layer::Overlay)
                }) {
                    if layer.cached_state().keyboard_interactivity
                        == KeyboardInteractivity::OnDemand
                    {
                        focus_layer = Some(layer.clone());
                    }
                };
                drop(layer_map);

                if let Some(layer) = focus_layer {
                    self.set_keyboard_focus(Some(layer));
                }

                // The only remaining case is LockSurface, which is already handled
            }
            PointerFocusTarget::Window(window) => {
                _ = self.fht.space.activate_window(&window, true);
            }
            PointerFocusTarget::LayerSurface(layer_surface) => {
                let mut focus = false;
                let layer_map = layer_map_for_output(output);
                if matches!(layer_surface.layer(), Layer::Top | Layer::Overlay)
                    && layer_surface.cached_state().keyboard_interactivity
                        == KeyboardInteractivity::OnDemand
                {
                    focus = true;
                }
                drop(layer_map);

                if focus {
                    self.set_keyboard_focus(Some(layer_surface));
                }
            }
        }
    }

    pub fn set_keyboard_focus(&mut self, ft: Option<impl Into<KeyboardFocusTarget>>) {
        let ft = ft.map(Into::into);
        self.fht
            .keyboard
            .clone()
            .set_focus(self, ft, SERIAL_COUNTER.next_serial());
    }

    pub fn move_pointer(&mut self, point: Point<f64, Logical>) {
        let pointer = self.fht.pointer.clone();
        let under = self.fht.focus_target_under(point);
        if self.fht.config.general.focus_follows_mouse && !pointer.is_grabbed() {
            self.handle_focus_follows_mouse(under.as_ref().map(|(p, _)| p).cloned());
        }

        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: point,
                serial: SERIAL_COUNTER.next_serial(),
                time: {
                    let duration: std::time::Duration = self.fht.clock.now().into();
                    duration.as_millis() as u32
                },
            },
        );
        self.fht.activate_pointer_constraint();

        pointer.frame(self);

        // FIXME: More granular, maybe check for where the point was and is now
        self.fht.queue_redraw_all();
    }

    #[rustfmt::skip]
    pub fn process_input_event<B: InputBackend>(&mut self, event: InputEvent<B>) {
        crate::profile_function!();

        // The spec doesn't specify which events should give activity, so just notify whenever
        // there's something that happened from the user.
        self.fht.idle_notify_activity();

        let should_enable_outputs = match &event {
            // Don't turn outputs on key releases, in case the user uses a keybind to disable
            // outputs, this would lead to us handling the key release that follow it, hence
            // re-enabling them.
            InputEvent::Keyboard { event } => event.state() == KeyState::Pressed,
            InputEvent::PointerButton { event } => event.state() == ButtonState::Pressed,
            // In the same logic, only device added events should be handled.
            InputEvent::DeviceAdded { .. } => true,
            // General motion events should always enable outputs.
            InputEvent::PointerMotion { .. }
            | InputEvent::PointerMotionAbsolute { .. }
            | InputEvent::PointerAxis { .. }
            | InputEvent::GestureSwipeBegin { .. }
            | InputEvent::GesturePinchBegin { .. }
            | InputEvent::GestureHoldBegin { .. }
            | InputEvent::TouchDown { .. }
            | InputEvent::TouchMotion { .. }
            | InputEvent::TabletToolAxis { .. }
            | InputEvent::TabletToolProximity { .. }
            | InputEvent::TabletToolTip { .. }
            | InputEvent::TabletToolButton { .. } => true,
            // Device removed/motion/gesture end events should not trigger.
            _ => false
        };
        if should_enable_outputs {
            self.fht.enable_outputs();
        }

        match event {
            InputEvent::DeviceAdded { device }          => self.on_device_added::<B>(device),
            InputEvent::DeviceRemoved { device }        => self.on_device_removed::<B>(device),
            InputEvent::Keyboard { event }              => self.on_keyboard_input::<B>(event),
            InputEvent::PointerMotion { event }         => self.on_pointer_motion::<B>(event),
            InputEvent::PointerMotionAbsolute { event } => self.on_pointer_motion_absolute::<B>(event),
            InputEvent::PointerButton { event }         => self.on_pointer_button::<B>(event),
            InputEvent::PointerAxis { event }           => self.on_pointer_axis::<B>(event),
            InputEvent::TabletToolAxis { event }        => self.on_tablet_tool_axis::<B>(event),
            InputEvent::TabletToolProximity { event }   => self.on_tablet_tool_proximity::<B>(event),
            InputEvent::TabletToolTip { event }         => self.on_tablet_tool_tip::<B>(event),
            InputEvent::TabletToolButton { event }      => self.on_tablet_tool_button::<B>(event),
            InputEvent::GestureSwipeBegin { event }     => self.on_gesture_swipe_begin::<B>(event),
            InputEvent::GestureSwipeUpdate { event }    => self.on_gesture_swipe_update::<B>(event),
            InputEvent::GestureSwipeEnd { event }       => self.on_gesture_swipe_end::<B>(event),
            InputEvent::GesturePinchBegin { event }     => self.on_gesture_pinch_begin::<B>(event),
            InputEvent::GesturePinchUpdate { event }    => self.on_gesture_pinch_update::<B>(event),
            InputEvent::GesturePinchEnd { event }       => self.on_gesture_pinch_end::<B>(event),
            InputEvent::GestureHoldBegin { event }      => self.on_gesture_hold_begin::<B>(event),
            InputEvent::GestureHoldEnd { event }        => self.on_gesture_hold_end::<B>(event),
            _ => {}
        }

        // FIXME: Granular
        self.fht.queue_redraw_all();
    }

    fn on_device_added<B: InputBackend>(&mut self, device: B::Device) {
        if device.has_capability(DeviceCapability::TabletTool) {
            self.fht
                .seat
                .tablet_seat()
                .add_tablet::<State>(&self.fht.display_handle, &TabletDescriptor::from(&device));
        }
    }

    fn on_device_removed<B: InputBackend>(&mut self, device: B::Device) {
        if device.has_capability(DeviceCapability::TabletTool) {
            let tablet_seat = self.fht.seat.tablet_seat();
            tablet_seat.remove_tablet(&TabletDescriptor::from(&device));
            // No tablets? then just remove all associated tools.
            if tablet_seat.count_tablets() == 0 {
                tablet_seat.clear_tools();
            }
        }
    }

    fn on_keyboard_input<B: InputBackend>(&mut self, event: B::KeyboardKeyEvent) {
        let keycode = event.key_code();
        let key_state: KeyState = event.state();
        let serial = SERIAL_COUNTER.next_serial();
        let time = event.time_msec();
        let keyboard = self.fht.keyboard.clone();

        // First candidate: Top/Overlay layershells asking for **Exclusive** keyboard
        // interaction They basically grab the keyboard, blocking every
        // other window from receiving input
        //
        // NOTE: We are checking from the topmost Overlay layer shell down to the lowest Top
        // layer shell
        for layer in self.fht.layer_shell_state.layer_surfaces().rev() {
            let (keyboard_interactivity, wlr_layer) =
                layer.with_cached_state(|state| (state.keyboard_interactivity, state.layer));
            if keyboard_interactivity == KeyboardInteractivity::Exclusive
                && matches!(wlr_layer, Layer::Top | Layer::Overlay)
            {
                let surface = self.fht.space.outputs().find_map(|o| {
                    let layer_map = layer_map_for_output(o);
                    let cloned = layer_map
                        .layers()
                        .find(|l| l.layer_surface() == &layer)
                        .cloned();
                    cloned
                });
                if let Some(surface) = surface {
                    self.set_keyboard_focus(Some(surface));
                    keyboard.input::<(), _>(self, keycode, key_state, serial, time, |_, _, _| {
                        FilterResult::Forward
                    });
                    return;
                }
            }
        }

        let pointer_location = self.fht.pointer.current_location();
        let inhibited = self
            .fht
            .focus_target_under(pointer_location)
            .and_then(|(ft, _)| {
                if let PointerFocusTarget::Window(w) = ft {
                    let wl_surface = w.wl_surface()?;
                    self.fht
                        .seat
                        .keyboard_shortcuts_inhibitor_for_surface(&wl_surface)
                } else {
                    None
                }
            })
            .map(|inhibitor| inhibitor.is_active())
            .unwrap_or(false);
        let action = keyboard.input(
            self,
            keycode,
            key_state,
            serial,
            time,
            |state, modifiers, handle| {
                // It has been proven to me that some people rather have their keybinds be affected
                // by the layout they are currently using, this does mean that you'd have to adapt
                // your keybinds in the config based on your layout.
                let modified = handle.modified_sym();
                let raw = handle.raw_latin_sym_or_raw_current_sym();

                // We handled a virtual terminal switch, no need to handle other keybinds.
                if handle_vt_switch(&mut state.backend, key_state, modified) {
                    state.fht.suppressed_keys.insert(modified);
                    return FilterResult::Intercept((KeyPattern::default(), KeyAction::none()));
                }

                if inhibited {
                    // FIXME: Add a way to override this for specific keybinds (this could be quite
                    // bad if you couldn't reload your compositor config or quit)
                    return FilterResult::Forward;
                }

                // Handle repeating keybinds
                // FIXME: handle this properly, this breaks if there are two repeating keybinds
                if key_state == KeyState::Released {
                    if let Some((token, _)) = state
                        .fht
                        .repeated_keyaction_timer
                        .take_if(|(_, k)| Some(*k) == raw)
                    {
                        state.fht.loop_handle.remove(token);
                    }
                }

                handle_key_action(
                    &state.fht.config.keybinds,
                    &mut state.fht.suppressed_keys,
                    modifiers,
                    key_state,
                    raw,
                )
            },
        );

        if let Some((key_pattern, key_action)) = action {
            self.process_key_action(key_action, key_pattern);
        }
    }

    fn on_pointer_motion<B: InputBackend>(&mut self, event: B::PointerMotionEvent) {
        let pointer = self.fht.pointer.clone();
        let mut pointer_location = pointer.current_location();
        let under = self.fht.focus_target_under(pointer_location);
        let serial = SERIAL_COUNTER.next_serial();

        let mut pointer_locked = false;
        let mut confine_region = None;

        if let Some((wl_surface, &surface_loc)) = under
            .as_ref()
            .and_then(|(ft, l)| Some((ft.wl_surface()?, l)))
        {
            with_pointer_constraint(&wl_surface, &pointer, |constraint| {
                match constraint {
                    Some(constraint) if constraint.is_active() => {
                        // Constraint basically useless if not within region/doesn't have a
                        // defined region
                        if !constraint.region().is_none_or(|region| {
                            region.contains((pointer_location - surface_loc).to_i32_round())
                        }) {
                            return;
                        }

                        match &*constraint {
                            PointerConstraint::Locked(_) => pointer_locked = true,
                            PointerConstraint::Confined(confine) => {
                                confine_region = confine.region().cloned();
                            }
                        }
                    }
                    _ => {}
                }
            });
        }

        pointer.relative_motion(
            self,
            under.clone(),
            &RelativeMotionEvent {
                delta: event.delta(),
                delta_unaccel: event.delta_unaccel(),
                utime: event.time(),
            },
        );

        if pointer_locked {
            // Pointer locked, don't emit motion event
            pointer.frame(self);
            return;
        }

        let mut new_pos = pointer_location + event.delta();
        let inside_space = self
            .fht
            .space
            .outputs()
            .all(|o| o.geometry().to_f64().contains(new_pos));

        // Here, we must properly clamp the new pointer location into the nearest output after
        // it has moved, in case for example, the user didn't configure proper gaps in their
        // output layout.
        if !inside_space {
            let rects: Vec<_> = self.fht.space.outputs().map(OutputExt::geometry).collect();
            let nearest = rects.into_iter().map(|rect| {
                let constrained = new_pos.constrain(rect.to_f64());
                (rect, constrained.x, constrained.y)
            });

            let (pos_x, pos_y) = new_pos.into();
            // Then, we try to find the nearest clamped position
            let nearest_clamp = nearest.min_by(|(_, x1, y1), (_, x2, y2)| {
                let dx1 = pos_x - x1;
                let dx2 = pos_x - x2;
                let dy1 = pos_y - y1;
                let dy2 = pos_y - y2;
                let d1 = f64::hypot(dx1, dy1);
                let d2 = f64::hypot(dx2, dy2);

                f64::total_cmp(&d1, &d2)
            });

            new_pos = nearest_clamp
                .map(|(rect, mut x, mut y)| {
                    let rect = rect.to_f64();
                    // Due to how space logic is done, we should not put the point at the
                    x = f64::min(x, rect.loc.x + rect.size.w - 1.0);
                    y = f64::min(y, rect.loc.y + rect.size.h - 1.0);
                    (x, y).into()
                })
                .unwrap_or(new_pos)
        }
        pointer_location = new_pos;

        let new_under = self.fht.focus_target_under(pointer_location);
        let maybe_new_output = self
            .fht
            .space
            .outputs()
            .find(|output| output.geometry().to_f64().contains(pointer_location))
            .cloned();
        if let Some(new_output) = maybe_new_output {
            self.fht.space.set_active_output(&new_output);
        }

        // Confine pointer if possible.
        if confine_region.is_some() {
            if let Some((ft, loc)) = &under {
                if new_under
                    .as_ref()
                    .and_then(|(new_ft, _)| new_ft.wl_surface())
                    != ft.wl_surface()
                {
                    pointer.frame(self);
                    return;
                }
                if confine_region.is_some_and(|region| {
                    !region.contains((pointer_location - *loc).to_i32_round())
                }) {
                    pointer.frame(self);
                    return;
                }
            }
        }

        if self.fht.config.general.focus_follows_mouse && !pointer.is_grabbed() {
            self.handle_focus_follows_mouse(under.as_ref().map(|(p, _)| p).cloned());
        }

        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pointer_location,
                serial,
                time: event.time_msec(),
            },
        );
        pointer.frame(self);

        // Try to activate new pointer constraint, if any.
        self.fht.activate_pointer_constraint();
    }

    fn on_pointer_motion_absolute<B: InputBackend>(
        &mut self,
        event: B::PointerMotionAbsoluteEvent,
    ) {
        let output_geo = self.fht.space.active_output().geometry();
        let pointer_location =
            event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
        let serial = SERIAL_COUNTER.next_serial();

        let pointer = self.fht.pointer.clone();
        let under = self.fht.focus_target_under(pointer_location);
        if self.fht.config.general.focus_follows_mouse && !pointer.is_grabbed() {
            self.handle_focus_follows_mouse(under.as_ref().map(|(p, _)| p).cloned());
        }

        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pointer_location,
                serial,
                time: event.time_msec(),
            },
        );
        pointer.frame(self);

        // Try to activate new pointer constraint, if any.
        self.fht.activate_pointer_constraint();
    }

    fn on_pointer_button<B: InputBackend>(&mut self, event: B::PointerButtonEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let button = event.button_code();
        let state = wl_pointer::ButtonState::from(event.state());
        let pointer = self.fht.pointer.clone();

        if state == wl_pointer::ButtonState::Pressed && !pointer.is_grabbed() {
            let pointer_loc = pointer.current_location();

            let mut has_layer_under = false;
            if let Some((PointerFocusTarget::LayerSurface(layer), _)) =
                self.fht.focus_target_under(pointer_loc)
            {
                if matches!(layer.layer(), Layer::Top | Layer::Overlay) {
                    has_layer_under = true;
                    self.fht.set_on_demand_layer_shell_focus(Some(&layer));
                }
            }

            if !has_layer_under {
                if let Some((window, _)) = self.fht.space.window_under(pointer_loc) {
                    // Activate the window so that on the next State::dispatch,
                    // update_keyboard_focus will focus the correct
                    // window
                    self.fht.space.activate_window(&window, true);
                }
            }

            if let Some(button) = event.button() {
                let mouse_pattern = fht_compositor_config::MousePattern(
                    self.fht.keyboard.modifier_state().into(),
                    button.into(),
                );
                if let Some(action) = self.fht.config.mousebinds.get(&mouse_pattern).cloned() {
                    self.process_mouse_action(event.button_code(), action, serial);
                }
            }
        }

        pointer.button(
            self,
            &ButtonEvent {
                button,
                state: state.try_into().unwrap(),
                serial,
                time: event.time_msec(),
            },
        );
        pointer.frame(self);
    }

    fn on_pointer_axis<B: InputBackend>(&mut self, event: B::PointerAxisEvent) {
        let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
        let vertical_amount_discrete = event.amount_v120(Axis::Vertical);
        let horizontal_amount = event
            .amount(Axis::Horizontal)
            .unwrap_or_else(|| horizontal_amount_discrete.unwrap_or(0.0) * 3.0 / 120.0);
        let vertical_amount = event
            .amount(Axis::Vertical)
            .unwrap_or_else(|| vertical_amount_discrete.unwrap_or(0.0) * 3.0 / 120.0);

        // Check for mouse axis bindings FIRST
        let modifiers = self.fht.keyboard.modifier_state().into();
        let mut handled = false;

        // Check vertical axis bindings using discrete amounts
        if let Some(discrete) = vertical_amount_discrete {
            if discrete > 0.0 {
                let mouse_pattern = fht_compositor_config::MousePattern(
                    modifiers,
                    fht_compositor_config::MouseInput::Axis(
                        fht_compositor_config::MouseAxis::WheelUp,
                    ),
                );
                if let Some(action) = self.fht.config.mousebinds.get(&mouse_pattern).cloned() {
                    self.process_mouse_action(0, action, SERIAL_COUNTER.next_serial());
                    handled = true;
                }
            } else if discrete < 0.0 {
                let mouse_pattern = fht_compositor_config::MousePattern(
                    modifiers,
                    fht_compositor_config::MouseInput::Axis(
                        fht_compositor_config::MouseAxis::WheelDown,
                    ),
                );
                if let Some(action) = self.fht.config.mousebinds.get(&mouse_pattern).cloned() {
                    self.process_mouse_action(0, action, SERIAL_COUNTER.next_serial());
                    handled = true;
                }
            }
        }

        // Check horizontal axis bindings using discrete amounts
        if !handled {
            if let Some(discrete) = horizontal_amount_discrete {
                if discrete > 0.0 {
                    let mouse_pattern = fht_compositor_config::MousePattern(
                        modifiers,
                        fht_compositor_config::MouseInput::Axis(
                            fht_compositor_config::MouseAxis::WheelRight,
                        ),
                    );
                    if let Some(action) = self.fht.config.mousebinds.get(&mouse_pattern).cloned() {
                        self.process_mouse_action(0, action, SERIAL_COUNTER.next_serial());
                        handled = true;
                    }
                } else if discrete < 0.0 {
                    let mouse_pattern = fht_compositor_config::MousePattern(
                        modifiers,
                        fht_compositor_config::MouseInput::Axis(
                            fht_compositor_config::MouseAxis::WheelLeft,
                        ),
                    );
                    if let Some(action) = self.fht.config.mousebinds.get(&mouse_pattern).cloned() {
                        self.process_mouse_action(0, action, SERIAL_COUNTER.next_serial());
                        handled = true;
                    }
                }
            }
        }

        // Only forward to pointer if we didn't handle it with a binding
        if !handled {
            let mut frame = AxisFrame::new(event.time_msec()).source(event.source());

            if horizontal_amount != 0.0 {
                frame = frame.relative_direction(
                    Axis::Horizontal,
                    event.relative_direction(Axis::Horizontal),
                );
                frame = frame.value(Axis::Horizontal, horizontal_amount);
                if let Some(discrete) = horizontal_amount_discrete {
                    frame = frame.v120(Axis::Horizontal, discrete as i32);
                }
            }

            if vertical_amount != 0.0 {
                frame = frame
                    .relative_direction(Axis::Vertical, event.relative_direction(Axis::Vertical));
                frame = frame.value(Axis::Vertical, vertical_amount);
                if let Some(discrete) = vertical_amount_discrete {
                    frame = frame.v120(Axis::Vertical, discrete as i32);
                }
            }

            if event.source() == AxisSource::Finger {
                if event.amount(Axis::Horizontal) == Some(0.0) {
                    frame = frame.stop(Axis::Horizontal);
                }
                if event.amount(Axis::Vertical) == Some(0.0) {
                    frame = frame.stop(Axis::Vertical);
                }
            }

            let pointer = self.fht.pointer.clone();
            pointer.axis(self, frame);
            pointer.frame(self);
        }
    }

    fn on_tablet_tool_axis<B: InputBackend>(&mut self, event: B::TabletToolAxisEvent) {
        let tablet_seat = self.fht.seat.tablet_seat();
        let Some(output_geometry) = self.fht.space.outputs().next().map(OutputExt::geometry) else {
            return;
        };

        let pointer_location =
            event.position_transformed(output_geometry.size) + output_geometry.loc.to_f64();

        let pointer = self.fht.pointer.clone();
        let under = self.fht.focus_target_under(pointer_location);
        let tablet = tablet_seat.get_tablet(&TabletDescriptor::from(&event.device()));
        let tool = tablet_seat.get_tool(&event.tool());

        pointer.motion(
            self,
            under.clone(),
            &MotionEvent {
                location: pointer_location,
                serial: SERIAL_COUNTER.next_serial(),
                time: 0,
            },
        );

        if let (Some(tablet), Some(tool)) = (tablet, tool) {
            if event.pressure_has_changed() {
                tool.pressure(event.pressure());
            }
            if event.distance_has_changed() {
                tool.distance(event.distance());
            }
            if event.tilt_has_changed() {
                tool.tilt(event.tilt());
            }
            if event.slider_has_changed() {
                tool.slider_position(event.slider_position());
            }
            if event.rotation_has_changed() {
                tool.rotation(event.rotation());
            }
            if event.wheel_has_changed() {
                tool.wheel(event.wheel_delta(), event.wheel_delta_discrete());
            }

            if let Some(under_with_loc) = under
                .clone()
                .and_then(|(f, loc)| f.wl_surface().map(|s| (s.into_owned(), loc)))
            {
                tool.motion(
                    pointer_location,
                    Some(under_with_loc),
                    &tablet,
                    SERIAL_COUNTER.next_serial(),
                    event.time_msec(),
                );
            } else {
                tool.motion(
                    pointer_location,
                    under.and_then(|(f, loc)| f.wl_surface().map(|s| (s.into_owned(), loc))),
                    &tablet,
                    SERIAL_COUNTER.next_serial(),
                    event.time_msec(),
                );
            }
        }
        pointer.frame(self);
    }

    fn on_tablet_tool_proximity<B: InputBackend>(&mut self, event: B::TabletToolProximityEvent) {
        let tablet_seat = self.fht.seat.tablet_seat();

        let Some(output_geo) = self.fht.space.outputs().next().map(OutputExt::geometry) else {
            return;
        };

        let tool = event.tool();
        let dh = self.fht.display_handle.clone();
        tablet_seat.add_tool::<Self>(self, &dh, &tool);

        let pointer_location =
            event.position_transformed(output_geo.size) + output_geo.loc.to_f64();

        let pointer = self.fht.pointer.clone();
        let under = self.fht.focus_target_under(pointer_location);
        let tablet = tablet_seat.get_tablet(&TabletDescriptor::from(&event.device()));
        let tool = tablet_seat.get_tool(&tool);

        pointer.motion(
            self,
            under.clone(),
            &MotionEvent {
                location: pointer_location,
                serial: SERIAL_COUNTER.next_serial(),
                time: 0,
            },
        );
        pointer.frame(self);

        let under = under.and_then(|(f, loc)| f.wl_surface().map(|s| (s.into_owned(), loc)));

        if let (Some(under), Some(tablet), Some(tool)) = (under, tablet, tool) {
            match event.state() {
                ProximityState::In => tool.proximity_in(
                    pointer_location,
                    under,
                    &tablet,
                    SERIAL_COUNTER.next_serial(),
                    event.time_msec(),
                ),
                ProximityState::Out => tool.proximity_out(event.time_msec()),
            }
        }
    }

    fn on_tablet_tool_tip<B: InputBackend>(&mut self, event: B::TabletToolTipEvent) {
        let tool = self.fht.seat.tablet_seat().get_tool(&event.tool());

        if let Some(tool) = tool {
            match event.tip_state() {
                TabletToolTipState::Down => {
                    let serial = SERIAL_COUNTER.next_serial();
                    tool.tip_down(serial, event.time_msec());
                    // change the keyboard focus
                    self.update_keyboard_focus();
                }
                TabletToolTipState::Up => {
                    tool.tip_up(event.time_msec());
                }
            }
        }
    }

    fn on_tablet_tool_button<B: InputBackend>(&mut self, event: B::TabletToolButtonEvent) {
        if let Some(tool) = self.fht.seat.tablet_seat().get_tool(&event.tool()) {
            tool.button(
                event.button(),
                event.button_state(),
                SERIAL_COUNTER.next_serial(),
                event.time_msec(),
            );
        }
    }

    fn on_gesture_swipe_begin<B: InputBackend>(&mut self, event: B::GestureSwipeBeginEvent) {
        self.fht.current_swipe_fingers = Some(event.fingers());
        self.fht.gesture_action_executed = false;

        let active_monitor = self.fht.space.active_monitor_mut();
        active_monitor.start_swipe_gesture(&self.fht.config.animations.workspace_switch);

        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.fht.pointer.clone();
        pointer.gesture_swipe_begin(
            self,
            &pointer::GestureSwipeBeginEvent {
                serial,
                time: event.time_msec(),
                fingers: event.fingers(),
            },
        );
    }

    fn on_gesture_swipe_update<B: InputBackend>(&mut self, event: B::GestureSwipeUpdateEvent) {
        let fingers = self.fht.current_swipe_fingers.unwrap_or(0);
        let delta = event.delta();

        let active_monitor = self.fht.space.active_monitor_mut();

        // If we don't have a swipe state, just forward the event to the client
        let Some(swipe_state) = active_monitor.swipe_state.as_ref() else {
            let pointer = self.fht.pointer.clone();
            pointer.gesture_swipe_update(
                self,
                &pointer::GestureSwipeUpdateEvent {
                    time: event.time_msec(),
                    delta: event.delta(),
                },
            );
            return;
        };

        let total_distance =
            (swipe_state.total_offset.x.powi(2) + swipe_state.total_offset.y.powi(2)).sqrt();

        // Detection of gesture direction
        let detected_direction = if let Some(dir) = swipe_state.direction {
            dir // Already determined
        } else if total_distance > swipe_state.direction_detection_threshold {
            if swipe_state.total_offset.x.abs() > swipe_state.total_offset.y.abs() {
                // Horizontal
                if swipe_state.total_offset.x > 0.0 {
                    GestureDirection::Right
                } else {
                    GestureDirection::Left
                }
            } else {
                // Vertical
                if swipe_state.total_offset.y > 0.0 {
                    GestureDirection::Down
                } else {
                    GestureDirection::Up
                }
            }
        } else {
            let active_monitor = self.fht.space.active_monitor_mut();
            active_monitor.update_swipe_gesture(
                Point::from((delta.x, delta.y)),
                Duration::from_millis(event.time_msec() as u64),
                None,
            );

            // We also transfer to the client while waiting
            let pointer = self.fht.pointer.clone();
            pointer.gesture_swipe_update(
                self,
                &pointer::GestureSwipeUpdateEvent {
                    time: event.time_msec(),
                    delta: event.delta(),
                },
            );
            return;
        };

        let mut action_handled = false;

        if !self.fht.gesture_action_executed {
            for (action, pattern) in &self.fht.config.gesturebinds {
                if pattern.fingers != fingers {
                    continue;
                }

                if total_distance < (pattern.min_swipe_distance as f64) {
                    continue;
                }

                if pattern.direction != GestureDirection::None
                    && pattern.direction != detected_direction
                {
                    continue;
                }

                action_handled = true;

                match action {
                    // Special case: Workspace Switch
                    GestureAction::FocusNextWorkspace | GestureAction::FocusPreviousWorkspace => {
                        // We check that the config is logical
                        let valid_action = match (action, detected_direction) {
                            (GestureAction::FocusNextWorkspace, GestureDirection::Left) => true,
                            (GestureAction::FocusNextWorkspace, GestureDirection::Up) => true,
                            (GestureAction::FocusPreviousWorkspace, GestureDirection::Right) => {
                                true
                            }
                            (GestureAction::FocusPreviousWorkspace, GestureDirection::Down) => true,
                            _ => false, /* Ex: bind "next" sur un swipe "right" (pas
                                         * logique) */
                        };

                        if valid_action {
                            let current_ws_idx = active_monitor.active_idx;
                            let at_limit = match detected_direction {
                                GestureDirection::Left | GestureDirection::Up => {
                                    current_ws_idx >= 8
                                }
                                GestureDirection::Right | GestureDirection::Down => {
                                    current_ws_idx == 0
                                }
                                _ => false,
                            };

                            if !at_limit {
                                let active_monitor = self.fht.space.active_monitor_mut();
                                active_monitor.update_swipe_gesture(
                                    Point::from((delta.x, delta.y)),
                                    Duration::from_millis(event.time_msec() as u64),
                                    Some(detected_direction),
                                );
                                self.fht.queue_redraw_all();
                            }
                        }
                    }

                    // Cas généraux : toutes les autres actions
                    _ => {
                        self.process_gesture_action(action.clone());
                        self.fht.gesture_action_executed = true;
                    }
                }

                break;
            }
        }

        if !action_handled {
            let pointer = self.fht.pointer.clone();
            pointer.gesture_swipe_update(
                self,
                &pointer::GestureSwipeUpdateEvent {
                    time: event.time_msec(),
                    delta: event.delta(),
                },
            );
        }
    }

    fn on_gesture_swipe_end<B: InputBackend>(&mut self, event: B::GestureSwipeEndEvent) {
        let active_monitor = self.fht.space.active_monitor_mut();
        if let Some(action) = active_monitor.end_swipe_gesture() {
            self.process_gesture_action(action);
        }

        self.fht.current_swipe_fingers = None;
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.fht.pointer.clone();
        pointer.gesture_swipe_end(
            self,
            &pointer::GestureSwipeEndEvent {
                serial,
                time: event.time_msec(),
                cancelled: event.cancelled(),
            },
        );
    }

    fn on_gesture_pinch_begin<B: InputBackend>(&mut self, event: B::GesturePinchBeginEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.fht.pointer.clone();
        pointer.gesture_pinch_begin(
            self,
            &pointer::GesturePinchBeginEvent {
                serial,
                time: event.time_msec(),
                fingers: event.fingers(),
            },
        )
    }

    fn on_gesture_pinch_update<B: InputBackend>(&mut self, event: B::GesturePinchUpdateEvent) {
        let pointer = self.fht.pointer.clone();
        pointer.gesture_pinch_update(
            self,
            &pointer::GesturePinchUpdateEvent {
                time: event.time_msec(),
                delta: GesturePinchUpdateEvent::delta(&event),
                scale: GesturePinchUpdateEvent::scale(&event),
                rotation: GesturePinchUpdateEvent::rotation(&event),
            },
        )
    }

    fn on_gesture_pinch_end<B: InputBackend>(&mut self, event: B::GesturePinchEndEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.fht.pointer.clone();
        pointer.gesture_pinch_end(
            self,
            &pointer::GesturePinchEndEvent {
                serial,
                time: event.time_msec(),
                cancelled: event.cancelled(),
            },
        )
    }

    fn on_gesture_hold_begin<B: InputBackend>(&mut self, event: B::GestureHoldBeginEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.fht.pointer.clone();
        pointer.gesture_hold_begin(
            self,
            &pointer::GestureHoldBeginEvent {
                serial,
                time: event.time_msec(),
                fingers: event.fingers(),
            },
        )
    }

    fn on_gesture_hold_end<B: InputBackend>(&mut self, event: B::GestureHoldEndEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.fht.pointer.clone();
        pointer.gesture_hold_end(
            self,
            &pointer::GestureHoldEndEvent {
                serial,
                time: event.time_msec(),
                cancelled: event.cancelled(),
            },
        )
    }
}

/// Returns `true` if a VT switch key has been handled.
fn handle_vt_switch(backend: &mut Backend, key_state: KeyState, modified: Keysym) -> bool {
    use smithay::input::keyboard::keysyms::*;

    if key_state == KeyState::Released {
        // We only do VT switch on presses.
        return false;
    }

    #[cfg(feature = "udev-backend")]
    #[allow(irrefutable_let_patterns)]
    if let Backend::Udev(udev) = backend {
        let vt_num = match modified.raw() {
            modified @ KEY_XF86Switch_VT_1..=KEY_XF86Switch_VT_12 => {
                (modified - KEY_XF86Switch_VT_1 + 1) as i32
            }
            _ => return false,
        };

        udev.switch_vt(vt_num as i32);
        return true;
    }

    // If we fall here, it's either the udev backend is disabled, or it's enabled and we are not
    // running on it (IE. we are on winit)
    return false;
}

fn handle_key_action(
    keybinds: &HashMap<KeyPattern, fht_compositor_config::KeyActionDesc>,
    suppressed: &mut HashSet<Keysym>,
    modifiers: &ModifiersState,
    key_state: KeyState,
    keysym: Option<Keysym>,
) -> FilterResult<(KeyPattern, KeyAction)> {
    let Some(keysym) = keysym else {
        // I don't know how we can have this case for regular keyboards.
        return FilterResult::Forward;
    };

    let key_action = find_keyaction(keybinds, modifiers, keysym);
    if key_state == KeyState::Pressed {
        if let Some(res) = key_action {
            suppressed.insert(keysym);
            return FilterResult::Intercept(res);
        } else {
            // There's nothing matching here, we can forward to the client.
            //
            // This would be a good place to check for builtin keybinds, however we handle VT
            // switching earlier (before event calling this function)
            return FilterResult::Forward;
        }
    }

    // In this case, we are releasing the key, check if there wasn't a previous keybind, since we
    // should inhibit both the key down and key up event.
    if suppressed.remove(&keysym) {
        let key_pattern = key_action.map_or_else(Default::default, |(kp, _)| kp);
        return FilterResult::Intercept((key_pattern, KeyAction::none()));
    }

    FilterResult::Forward
}

/// Try to find a given key action from the given [`ModifiersState`] and [`Keysym`]
fn find_keyaction(
    keybinds: &HashMap<KeyPattern, fht_compositor_config::KeyActionDesc>,
    modifiers: &ModifiersState,
    keysym: Keysym,
) -> Option<(KeyPattern, KeyAction)> {
    let key_pattern = KeyPattern((*modifiers).into(), keysym);
    // NOTE: We don't filter for locked state and such here, it's handled when we are about to
    // actually execute the keybind, so that we still intercept the key.
    match keybinds.get(&key_pattern) {
        Some(key_action) => {
            trace!(?key_pattern, ?key_action, "Got matching key-action");
            Some((key_pattern, key_action.clone().into()))
        }
        None => None,
    }
}
