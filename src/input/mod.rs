pub mod actions;
pub mod pick_surface_grab;
pub mod resize_tile_grab;
pub mod swap_tile_grab;

pub use actions::*;
use fht_compositor_config::KeyPattern;
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, Device, DeviceCapability, Event, GestureBeginEvent,
    GestureEndEvent, GesturePinchUpdateEvent, GestureSwipeUpdateEvent, InputBackend, InputEvent,
    KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    ProximityState, TabletToolButtonEvent, TabletToolEvent, TabletToolProximityEvent,
    TabletToolTipEvent, TabletToolTipState,
};
use smithay::desktop::layer_map_for_output;
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{self, AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent};
use smithay::reexports::wayland_server::protocol::wl_pointer;
use smithay::utils::{IsAlive, Logical, Point, SERIAL_COUNTER};
use smithay::wayland::compositor::with_states;
use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat;
use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraint};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer, LayerSurfaceCachedState};
use smithay::wayland::tablet_manager::{TabletDescriptor, TabletSeatTrait};

use crate::focus_target::{KeyboardFocusTarget, PointerFocusTarget};
use crate::output::OutputExt;
use crate::state::State;

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
            self.update_keyboard_focus();
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

    pub fn process_input_event<B: InputBackend>(&mut self, event: InputEvent<B>) {
        crate::profile_function!();
        match event {
            InputEvent::DeviceAdded { device } => {
                if device.has_capability(DeviceCapability::TabletTool) {
                    self.fht.seat.tablet_seat().add_tablet::<State>(
                        &self.fht.display_handle,
                        &TabletDescriptor::from(&device),
                    );
                }
            }
            InputEvent::DeviceRemoved { device } => {
                if device.has_capability(DeviceCapability::TabletTool) {
                    let tablet_seat = self.fht.seat.tablet_seat();
                    tablet_seat.remove_tablet(&TabletDescriptor::from(&device));
                    // No tablets? then just remove all associated tools.
                    if tablet_seat.count_tablets() == 0 {
                        tablet_seat.clear_tools();
                    }
                }
            }
            InputEvent::Keyboard { event } => {
                let keycode = event.key_code();
                let key_state: KeyState = event.state();
                trace!(?keycode, ?key_state, "Key");
                let serial = SERIAL_COUNTER.next_serial();
                let time = event.time_msec();
                let keyboard = self.fht.keyboard.clone();

                let mut suppressed_keys = self.fht.suppressed_keys.clone();

                // First candidate: Top/Overlay layershells asking for **Exclusive** keyboard
                // interaction They basically grab the keyboard, blocking every
                // other window from receiving input
                //
                // NOTE: We are checking from the topmost Overlay layer shell down to the lowest Top
                // layer shell
                for layer in self.fht.layer_shell_state.layer_surfaces().rev() {
                    let data = with_states(layer.wl_surface(), |state| {
                        *state
                            .cached_state
                            .get::<LayerSurfaceCachedState>()
                            .current()
                    });
                    if data.keyboard_interactivity == KeyboardInteractivity::Exclusive
                        && (data.layer == Layer::Top || data.layer == Layer::Overlay)
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
                            keyboard.input::<(), _>(
                                self,
                                keycode,
                                key_state,
                                serial,
                                time,
                                |_, _, _| FilterResult::Forward,
                            );
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
                        // Use the first raw keysym
                        //
                        // What does this mean? Basically a modified sym would also apply
                        // modifiers to the final [`Keysym`], which isnt good for user
                        // interactivity since a [`KeyPattern`] with ALT+SHIFT+1 is not 1 but with
                        // bang since 1 capital on QWERTY is bang
                        //
                        // This also ignores non-qwerty keyboards too, I have to think about this
                        // sometime
                        let keysym = *handle.raw_syms().first().unwrap();

                        #[cfg(feature = "udev-backend")]
                        {
                            use smithay::input::keyboard::Keysym;
                            if key_state == KeyState::Pressed
                                && (Keysym::XF86_Switch_VT_1.raw()
                                    ..=Keysym::XF86_Switch_VT_12.raw())
                                    .contains(&handle.modified_sym().raw())
                            {
                                #[allow(irrefutable_let_patterns)]
                                if let crate::backend::Backend::Udev(data) = &mut state.backend {
                                    data.switch_vt(
                                        (handle.modified_sym().raw()
                                            - Keysym::XF86_Switch_VT_1.raw()
                                            + 1) as i32,
                                    );
                                    suppressed_keys.insert(keysym);
                                    return FilterResult::Intercept((
                                        KeyAction::none(),
                                        KeyPattern::default(),
                                    ));
                                }
                            }
                        }

                        #[allow(unused_mut)]
                        let mut modifiers = *modifiers;
                        // Swap ALT and SUPER under the winit backend since you are probably running
                        // under a parent compositor that already has binds with the super key.
                        #[cfg(feature = "winit-backend")]
                        if matches!(&mut state.backend, crate::backend::Backend::Winit(_)) {
                            modifiers = smithay::input::keyboard::ModifiersState {
                                alt: modifiers.logo,
                                logo: modifiers.alt,
                                ..modifiers
                            }
                        }

                        let key_pattern =
                            fht_compositor_config::KeyPattern(modifiers.into(), keysym);
                        if key_state == KeyState::Pressed && !inhibited {
                            let action = state
                                .fht
                                .config
                                .keybinds
                                .get(&key_pattern)
                                .cloned()
                                .map(Into::into);
                            trace!(?keysym, ?key_pattern, ?action);

                            if let Some(action) = action {
                                suppressed_keys.insert(keysym);
                                FilterResult::Intercept((action, key_pattern))
                            } else {
                                FilterResult::Forward
                            }
                        } else if suppressed_keys.remove(&keysym) {
                            // If the current repeat timer is for the following keysym, remove it
                            // FIXME: Check this logic since sometimes (for obscure reasons) there
                            // can be two keyactions running
                            if let Some((token, _)) = state
                                .fht
                                .repeated_keyaction_timer
                                .take_if(|(_, k)| *k == keysym)
                            {
                                state.fht.loop_handle.remove(token);
                            }

                            FilterResult::Intercept((KeyAction::none(), key_pattern))
                        } else {
                            FilterResult::Forward
                        }
                    },
                );

                self.fht.suppressed_keys = suppressed_keys;
                if let Some((action, key_pattern)) = action {
                    self.process_key_action(action, key_pattern);
                }
            }
            InputEvent::PointerMotion { event } => {
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
                if !self
                    .fht
                    .space
                    .outputs()
                    .any(|o| o.geometry().to_f64().contains(pointer_location))
                {
                    // Clamp the pointer location to the previous output
                    let previous_output = self.fht.space.active_output().clone();
                    let geometry = previous_output.geometry();
                    new_pos.x = new_pos.x.clamp(
                        geometry.loc.x as f64,
                        (geometry.loc.x + geometry.size.w - 1) as f64,
                    );
                    new_pos.y = new_pos.y.clamp(
                        geometry.loc.y as f64,
                        (geometry.loc.y + geometry.size.h - 1) as f64,
                    );
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
                    self.update_keyboard_focus();
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
            InputEvent::PointerMotionAbsolute { event } => {
                let output_geo = self.fht.space.active_output().geometry();
                let pointer_location =
                    event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
                let serial = SERIAL_COUNTER.next_serial();

                let pointer = self.fht.pointer.clone();
                let under = self.fht.focus_target_under(pointer_location);
                if self.fht.config.general.focus_follows_mouse && !pointer.is_grabbed() {
                    self.update_keyboard_focus();
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
            InputEvent::PointerButton { event } => {
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
                        if let Some(action) =
                            self.fht.config.mousebinds.get(&mouse_pattern).cloned()
                        {
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
            InputEvent::PointerAxis { event } => {
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
                
                // Check vertical axis bindings
                if vertical_amount > 0.0 {
                    let mouse_pattern = fht_compositor_config::MousePattern(
                        modifiers,
                        fht_compositor_config::MouseInput::Axis(fht_compositor_config::MouseAxis::WheelUp),
                    );
                    if let Some(action) = self.fht.config.mousebinds.get(&mouse_pattern).cloned() {
                        self.process_mouse_action(0, action, SERIAL_COUNTER.next_serial());
                        handled = true;
                    }
                } else if vertical_amount < 0.0 {
                    let mouse_pattern = fht_compositor_config::MousePattern(
                        modifiers,
                        fht_compositor_config::MouseInput::Axis(fht_compositor_config::MouseAxis::WheelDown),
                    );
                    if let Some(action) = self.fht.config.mousebinds.get(&mouse_pattern).cloned() {
                        self.process_mouse_action(0, action, SERIAL_COUNTER.next_serial());
                        handled = true;
                    }
                }
                
                // Check horizontal axis bindings
                if !handled && horizontal_amount > 0.0 {
                    let mouse_pattern = fht_compositor_config::MousePattern(
                        modifiers,
                        fht_compositor_config::MouseInput::Axis(fht_compositor_config::MouseAxis::WheelRight),
                    );
                    if let Some(action) = self.fht.config.mousebinds.get(&mouse_pattern).cloned() {
                        self.process_mouse_action(0, action, SERIAL_COUNTER.next_serial());
                        handled = true;
                    }
                } else if !handled && horizontal_amount < 0.0 {
                    let mouse_pattern = fht_compositor_config::MousePattern(
                        modifiers,
                        fht_compositor_config::MouseInput::Axis(fht_compositor_config::MouseAxis::WheelLeft),
                    );
                    if let Some(action) = self.fht.config.mousebinds.get(&mouse_pattern).cloned() {
                        self.process_mouse_action(0, action, SERIAL_COUNTER.next_serial());
                        handled = true;
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
                        frame = frame.relative_direction(
                            Axis::Vertical,
                            event.relative_direction(Axis::Vertical),
                        );
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
            InputEvent::TabletToolAxis { event } => {
                let tablet_seat = self.fht.seat.tablet_seat();
                let Some(output_geometry) =
                    self.fht.space.outputs().next().map(OutputExt::geometry)
                else {
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
                            under
                                .and_then(|(f, loc)| f.wl_surface().map(|s| (s.into_owned(), loc))),
                            &tablet,
                            SERIAL_COUNTER.next_serial(),
                            event.time_msec(),
                        );
                    }
                }
                pointer.frame(self);
            }
            InputEvent::TabletToolProximity { event } => {
                let tablet_seat = self.fht.seat.tablet_seat();

                let Some(output_geo) = self.fht.space.outputs().next().map(OutputExt::geometry)
                else {
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

                let under =
                    under.and_then(|(f, loc)| f.wl_surface().map(|s| (s.into_owned(), loc)));

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
            InputEvent::TabletToolTip { event } => {
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
            InputEvent::TabletToolButton { event } => {
                let tool = self.fht.seat.tablet_seat().get_tool(&event.tool());

                if let Some(tool) = tool {
                    tool.button(
                        event.button(),
                        event.button_state(),
                        SERIAL_COUNTER.next_serial(),
                        event.time_msec(),
                    );
                }
            }
            InputEvent::GestureSwipeBegin { event } => {
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
            InputEvent::GestureSwipeUpdate { event } => {
                let pointer = self.fht.pointer.clone();
                pointer.gesture_swipe_update(
                    self,
                    &pointer::GestureSwipeUpdateEvent {
                        time: event.time_msec(),
                        delta: GestureSwipeUpdateEvent::delta(&event),
                    },
                );
            }
            InputEvent::GestureSwipeEnd { event } => {
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
            InputEvent::GesturePinchBegin { event } => {
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
            InputEvent::GesturePinchUpdate { event } => {
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
            InputEvent::GesturePinchEnd { event } => {
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
            InputEvent::GestureHoldBegin { event } => {
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
            InputEvent::GestureHoldEnd { event } => {
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
            _ => {}
        }

        // FIXME: Granular
        self.fht.queue_redraw_all();
    }
}
