use smithay::input::keyboard::LedState;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::input::DeviceCapability;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::set_data_device_focus;
use smithay::wayland::selection::primary_selection::set_primary_focus;
use smithay::wayland::tablet_manager::TabletSeatHandler;
use smithay::{delegate_seat, delegate_tablet_manager, delegate_text_input_manager};

use crate::shell::{KeyboardFocusTarget, PointerFocusTarget};
use crate::state::State;

impl TabletSeatHandler for State {
    fn tablet_tool_image(
        &mut self,
        _tool: &smithay::backend::input::TabletToolDescriptor,
        image_status: smithay::input::pointer::CursorImageStatus,
    ) {
        if self.fht.resize_grab_active || self.fht.interactive_grab_active {
            // These interactive grabs set themselves a cursor icon.
            // Do not override it
            return;
        }

        self.fht.cursor_theme_manager.set_image_status(image_status);
    }
}

impl SeatHandler for State {
    type KeyboardFocus = KeyboardFocusTarget;
    type PointerFocus = PointerFocusTarget;
    type TouchFocus = PointerFocusTarget;

    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.fht.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&KeyboardFocusTarget>) {
        let dh = &self.fht.display_handle;
        let wl_surface = focused.and_then(WaylandFocus::wl_surface);
        let client = wl_surface.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);
    }

    fn led_state_changed(&mut self, _seat: &Seat<Self>, led_state: LedState) {
        let keyboards = self
            .fht
            .devices
            .iter()
            .filter(|device| device.has_capability(DeviceCapability::Keyboard))
            .cloned();

        for mut keyboard in keyboards {
            keyboard.led_update(led_state.into());
        }
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        image: smithay::input::pointer::CursorImageStatus,
    ) {
        self.fht.cursor_theme_manager.set_image_status(image);
    }
}

delegate_seat!(State);

delegate_tablet_manager!(State);

delegate_text_input_manager!(State);
