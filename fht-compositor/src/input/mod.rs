pub mod actions;

pub use actions::*;
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, Device, DeviceCapability, Event, GestureBeginEvent,
    GestureEndEvent, GesturePinchUpdateEvent, GestureSwipeUpdateEvent, InputBackend, InputEvent,
    KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    ProximityState, TabletToolButtonEvent, TabletToolEvent, TabletToolProximityEvent,
    TabletToolTipEvent, TabletToolTipState,
};
#[cfg(feature = "udev_backend")]
use smithay::backend::session::Session;
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::input::keyboard::{FilterResult, Keysym};
use smithay::input::pointer::{self, AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent};
use smithay::reexports::wayland_server::protocol::wl_pointer;
use smithay::utils::{Point, SERIAL_COUNTER};
use smithay::wayland::compositor::with_states;
use smithay::wayland::input_method::InputMethodSeat;
use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat;
use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraint};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer, LayerSurfaceCachedState};
use smithay::wayland::tablet_manager::{TabletDescriptor, TabletSeatTrait};

use crate::config::CONFIG;
use crate::shell::PointerFocusTarget;
use crate::state::{egui_state_for_output, State};
use crate::utils::geometry::{Global, PointExt, PointGlobalExt, PointLocalExt, RectGlobalExt};
use crate::utils::output::OutputExt;

impl State {
    #[profiling::function]
    fn update_keyboard_focus(&mut self) {
        let keyboard = self.fht.keyboard.clone();
        let pointer = self.fht.pointer.clone();
        let input_method = self.fht.seat.input_method();

        // Update the current keyboard focus if both the keyboard and the pointer are not grabbed.
        //
        // If the pointer/keyboard are grabbed by for example a subsurface/popup that's getting
        // dimissed while they still focus it will cause some trouble and issues (for example in
        // firefox-wayland
        //
        // https://gitlab.freedesktop.org/wayland/wayland/-/issues/294
        if pointer.is_grabbed() || keyboard.is_grabbed() || input_method.keyboard_grabbed() {
            return;
        }

        let Some(ref output) = self.fht.focus_state.output.clone() else {
            return;
        };

        let pointer_loc = pointer.current_location().as_global();
        let layer_map = layer_map_for_output(output);
        let wset = self.fht.wset_mut_for(output);
        let active = wset.active_mut();

        if let Some(layer) = layer_map.layer_under(Layer::Overlay, pointer_loc.as_logical()) {
            if layer.can_receive_keyboard_focus() {
                let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
                if layer
                    .surface_under(
                        pointer_loc.to_local(output).as_logical() - layer_loc.to_f64(),
                        WindowSurfaceType::ALL,
                    )
                    .is_some()
                {
                    self.fht.focus_state.focus_target = Some(layer.clone().into());
                    return;
                }
            }
        } else if let Some(fullscreen) = active.fullscreen.as_ref().map(|f| &f.inner) {
            if fullscreen
                .surface_under(
                    pointer_loc.to_local(output).as_logical(),
                    WindowSurfaceType::ALL,
                )
                .is_some()
            {
                self.fht.focus_state.focus_target = Some(fullscreen.clone().into());
                return;
            }
        } else if let Some(layer) = layer_map.layer_under(Layer::Top, pointer_loc.as_logical()) {
            if layer.can_receive_keyboard_focus() {
                let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
                if layer
                    .surface_under(
                        pointer_loc.to_local(output).as_logical() - layer_loc.to_f64(),
                        WindowSurfaceType::ALL,
                    )
                    .is_some()
                {
                    self.fht.focus_state.focus_target = Some(layer.clone().into());
                    return;
                }
            }
        } else if let Some(window) = active
            .window_under(pointer_loc)
            .filter(|(w, _)| {
                // Don't focus override redirect windows
                #[cfg(feature = "xwayland")]
                return !w.is_x11_override_redirect();
                #[cfg(not(feature = "xwayland"))]
                return true;
            })
            .map(|(w, _)| w.clone())
        {
            active.focus_window(&window);
            active.raise_window(&window);
            self.fht.focus_state.focus_target = Some(window.clone().into());
        } else if let Some(layer) = layer_map
            .layer_under(Layer::Bottom, pointer_loc.as_logical())
            .or_else(|| layer_map.layer_under(Layer::Background, pointer_loc.as_logical()))
        {
            if layer.can_receive_keyboard_focus() {
                let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
                if layer
                    .surface_under(
                        pointer_loc.to_local(output).as_logical() - layer_loc.to_f64(),
                        WindowSurfaceType::ALL,
                    )
                    .is_some()
                {
                    self.fht.focus_state.focus_target = Some(layer.clone().into());
                    return;
                }
            }
        }
    }

    pub fn move_pointer(&mut self, point: Point<f64, Global>) {
        let pointer = self.fht.pointer.clone();
        let under = self.fht.focus_target_under(point);

        if let Some(output) = self.fht.focus_state.output.as_ref() {
            let position = point.to_i32_round().to_local(output).as_logical();
            egui_state_for_output(output).handle_pointer_motion(position);
        }

        pointer.motion(
            self,
            under.map(|(ft, loc)| (ft, loc.as_logical())),
            &MotionEvent {
                location: point.as_logical(),
                serial: SERIAL_COUNTER.next_serial(),
                time: {
                    let duration: std::time::Duration = self.fht.clock.now().into();
                    duration.as_millis() as u32
                },
            },
        );
        pointer.frame(self);

        // FIXME: More granular, maybe check for where the point was and is now
        for output in self.fht.outputs() {
            self.backend
                .schedule_render_output(output, &self.fht.loop_handle);
        }
    }

    fn clamp_coords(&self, pos: Point<f64, Global>) -> Point<f64, Global> {
        let (pos_x, pos_y) = pos.into();
        let max_x = self
            .fht
            .outputs()
            .fold(0, |acc, o| acc + o.geometry().size.w);
        let clamped_x = pos_x.clamp(0.0, max_x as f64);
        let max_y = self
            .fht
            .outputs()
            .find(|o| o.geometry().contains((clamped_x as i32, 0)))
            .map(|o| o.geometry().size.h);

        if let Some(max_y) = max_y {
            let clamped_y = pos_y.clamp(0.0, max_y as f64);
            (clamped_x, clamped_y).into()
        } else {
            (clamped_x, pos_y).into()
        }
    }

    #[profiling::function]
    pub fn process_input_event<B: InputBackend>(&mut self, event: InputEvent<B>) {
        let mut output = self.fht.active_output();

        match event {
            InputEvent::DeviceAdded { device } => {
                if device.has_capability(DeviceCapability::TabletTool) {
                    self.fht.seat.tablet_seat().add_tablet::<State>(
                        &self.fht.display_handle,
                        &TabletDescriptor::from(&device),
                    );
                }
                // TODO: Handle touch devices.
                egui_state_for_output(&output).handle_device_added(&device);
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
                egui_state_for_output(&output).handle_device_removed(&device);
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
                        *state.cached_state.current::<LayerSurfaceCachedState>()
                    });
                    if data.keyboard_interactivity == KeyboardInteractivity::Exclusive
                        && (data.layer == Layer::Top || data.layer == Layer::Overlay)
                    {
                        let surface = self.fht.outputs().find_map(|o| {
                            let layer_map = layer_map_for_output(o);
                            let cloned = layer_map
                                .layers()
                                .find(|l| l.layer_surface() == &layer)
                                .cloned();
                            cloned
                        });
                        if let Some(surface) = surface {
                            keyboard.set_focus(self, Some(surface.into()), serial);
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

                let pointer_location = self.fht.pointer.current_location().as_global();
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

                        #[cfg(feature = "udev_backend")]
                        if key_state == KeyState::Pressed
                            && (Keysym::XF86_Switch_VT_1.raw()..=Keysym::XF86_Switch_VT_12.raw())
                                .contains(&handle.modified_sym().raw())
                        {
                            if let crate::backend::Backend::Udev(data) = &mut state.backend {
                                if let Err(err) = data.session.change_vt(
                                    (handle.modified_sym().raw() - Keysym::XF86_Switch_VT_1.raw()
                                        + 1) as i32,
                                ) {
                                    error!(?err, "Failed switching virtual terminal.");
                                }
                                suppressed_keys.insert(keysym);
                                return FilterResult::Intercept(KeyAction::None);
                            }
                        }

                        if key_state == KeyState::Pressed && !inhibited {
                            let key_pattern = KeyPattern((*modifiers).into(), keysym);
                            let action = CONFIG.keybinds.get(&key_pattern).cloned();
                            debug!(?keysym, ?key_pattern, ?action);

                            if let Some(action) = action {
                                suppressed_keys.insert(keysym);
                                FilterResult::Intercept(action)
                            } else {
                                FilterResult::Forward
                            }
                        } else if suppressed_keys.remove(&keysym) {
                            FilterResult::Intercept(KeyAction::None)
                        } else {
                            FilterResult::Forward
                        }
                    },
                );

                self.fht.suppressed_keys = suppressed_keys;
                if let Some(action) = action {
                    self.process_key_action(action);
                }
            }
            InputEvent::PointerMotion { event } => {
                let pointer = self.fht.pointer.clone();
                let mut pointer_location = pointer.current_location().as_global();
                let under = self.fht.focus_target_under(pointer_location);
                let serial = SERIAL_COUNTER.next_serial();

                let mut pointer_locked = false;
                let mut pointer_confined = false;
                let mut confine_region = None;

                if let Some((wl_surface, surface_loc)) = under
                    .as_ref()
                    .and_then(|(ft, l)| Some((ft.wl_surface()?, l)))
                {
                    with_pointer_constraint(&wl_surface, &pointer, |constraint| {
                        match constraint {
                            Some(constraint) if constraint.is_active() => {
                                // Constraint basically useless if not within region/doesn't have a
                                // defined region
                                if !constraint.region().map_or(true, |region| {
                                    region.contains(
                                        (pointer_location.to_i32_round() - *surface_loc)
                                            .as_logical(),
                                    )
                                }) {
                                    return;
                                }

                                match &*constraint {
                                    PointerConstraint::Locked(_) => pointer_locked = true,
                                    PointerConstraint::Confined(confine) => {
                                        pointer_confined = true;
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
                    under.clone().map(|(ft, loc)| (ft, loc.as_logical())),
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

                pointer_location += event.delta().as_global();
                pointer_location = self.clamp_coords(pointer_location);
                let new_under = self.fht.focus_target_under(pointer_location);

                let maybe_new_output = self
                    .fht
                    .outputs()
                    .find(|output| output.geometry().to_f64().contains(pointer_location))
                    .cloned();
                if let Some(new_output) = maybe_new_output {
                    self.fht.focus_state.output = Some(new_output.clone());
                    output = new_output;
                }

                // Confine pointer if possible.
                if pointer_confined {
                    if let Some((ft, loc)) = &under {
                        if new_under.as_ref().and_then(|(ft, _)| ft.wl_surface()) != ft.wl_surface()
                        {
                            pointer.frame(self);
                            return;
                        }
                        if confine_region.is_some_and(|region| {
                            region.contains((pointer_location.to_i32_round() - *loc).as_logical())
                        }) {
                            pointer.frame(self);
                            return;
                        }
                    }
                }

                pointer.motion(
                    self,
                    under.map(|(ft, loc)| (ft, loc.as_logical())),
                    &MotionEvent {
                        location: pointer_location.as_logical(),
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);

                {
                    let location = pointer_location
                        .to_local(&output)
                        .to_i32_round()
                        .as_logical();
                    egui_state_for_output(&output).handle_pointer_motion(location);
                }

                // If pointer is now in a constraint region, activate it
                // TODO: Anywhere else pointer is moved needs to do this (in the self.move_pointer
                // function)
                if let Some((under, surface_location)) =
                    new_under.and_then(|(target, loc)| Some((target.wl_surface()?, loc)))
                {
                    with_pointer_constraint(&under, &pointer, |constraint| match constraint {
                        Some(constraint) if !constraint.is_active() => {
                            let point = pointer_location.to_i32_round() - surface_location;
                            if constraint
                                .region()
                                .map_or(true, |region| region.contains(point.as_logical()))
                            {
                                constraint.activate();
                            }
                        }
                        _ => {}
                    });
                }
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let output_geo = output.geometry().as_logical();
                let pointer_location = (event.position_transformed(output_geo.size)
                    + output_geo.loc.to_f64())
                .as_global();
                let serial = SERIAL_COUNTER.next_serial();

                let pointer = self.fht.pointer.clone();
                let under = self.fht.focus_target_under(pointer_location);

                {
                    let local_pos = pointer_location
                        .to_i32_round()
                        .to_local(&output)
                        .as_logical();
                    egui_state_for_output(&output).handle_pointer_motion(local_pos);
                }

                pointer.motion(
                    self,
                    under.map(|(ft, loc)| (ft, loc.as_logical())),
                    &MotionEvent {
                        location: pointer_location.as_logical(),
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerButton { event } => {
                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();
                let state = wl_pointer::ButtonState::from(event.state());
                let pointer = self.fht.pointer.clone();

                {
                    let egui = egui_state_for_output(&output);
                    if egui.wants_pointer()
                        && let Some(button) = event.button()
                    {
                        egui.handle_pointer_button(
                            button,
                            state == wl_pointer::ButtonState::Pressed,
                        );
                        return;
                    }
                }

                if state == wl_pointer::ButtonState::Pressed {
                    self.update_keyboard_focus();

                    if let Some(button) = event.button() {
                        let mouse_pattern =
                            MousePattern(self.fht.keyboard.modifier_state().into(), button.into());
                        if let Some(action) = CONFIG.mousebinds.get(&mouse_pattern).cloned() {
                            self.process_mouse_action(action, serial);
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

                {
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
                        if let Some(discrete) = horizontal_amount_discrete {
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

                    {
                        let egui = egui_state_for_output(&output);
                        if egui.wants_pointer() {
                            egui.handle_pointer_axis(horizontal_amount, vertical_amount);
                            return;
                        }
                    }

                    let pointer = self.fht.pointer.clone();
                    pointer.axis(self, frame);
                    pointer.frame(self);
                }
            }
            InputEvent::TabletToolAxis { event } => {
                let tablet_seat = self.fht.seat.tablet_seat();
                let Some(output_geometry) = self
                    .fht
                    .outputs()
                    .next()
                    .map(OutputExt::geometry)
                    .map(|geo| geo.as_logical())
                else {
                    return;
                };

                let pointer_location = (event.position_transformed(output_geometry.size)
                    + output_geometry.loc.to_f64())
                .as_global();

                let pointer = self.fht.pointer.clone();
                let under = self.fht.focus_target_under(pointer_location);
                let tablet = tablet_seat.get_tablet(&TabletDescriptor::from(&event.device()));
                let tool = tablet_seat.get_tool(&event.tool());

                pointer.motion(
                    self,
                    under.clone().map(|(ft, loc)| (ft, loc.as_logical())),
                    &MotionEvent {
                        location: pointer_location.as_logical(),
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

                    tool.motion(
                        pointer_location.as_logical(),
                        under.and_then(|(f, loc)| f.wl_surface().map(|s| (s, loc.as_logical()))),
                        &tablet,
                        SERIAL_COUNTER.next_serial(),
                        event.time_msec(),
                    );
                }
                pointer.frame(self);
            }
            InputEvent::TabletToolProximity { event } => {
                let tablet_seat = self.fht.seat.tablet_seat();

                let Some(output_geo) = self
                    .fht
                    .outputs()
                    .next()
                    .map(OutputExt::geometry)
                    .map(|geo| geo.as_logical())
                else {
                    return;
                };

                let tool = event.tool();
                tablet_seat.add_tool::<Self>(&self.fht.display_handle, &tool);

                let pointer_location = (event.position_transformed(output_geo.size)
                    + output_geo.loc.to_f64())
                .as_global();

                let pointer = self.fht.pointer.clone();
                let under = self.fht.focus_target_under(pointer_location);
                let tablet = tablet_seat.get_tablet(&TabletDescriptor::from(&event.device()));
                let tool = tablet_seat.get_tool(&tool);

                pointer.motion(
                    self,
                    under.clone().map(|(ft, loc)| (ft, loc.as_logical())),
                    &MotionEvent {
                        location: pointer_location.as_logical(),
                        serial: SERIAL_COUNTER.next_serial(),
                        time: 0,
                    },
                );
                pointer.frame(self);

                if let (Some(under), Some(tablet), Some(tool)) = (
                    under.and_then(|(f, loc)| f.wl_surface().map(|s| (s, loc.as_logical()))),
                    tablet,
                    tool,
                ) {
                    match event.state() {
                        ProximityState::In => tool.proximity_in(
                            pointer_location.as_logical(),
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
        for output in self.fht.outputs() {
            self.backend
                .schedule_render_output(output, &self.fht.loop_handle);
        }
    }
}
