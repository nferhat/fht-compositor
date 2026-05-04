use smithay::delegate_data_device;
use smithay::input::dnd::{DnDGrab, DndGrabHandler, GrabType};
use smithay::input::pointer::Focus;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::selection::data_device::{DataDeviceHandler, WaylandDndGrabHandler};

use crate::state::State;

impl DataDeviceHandler for State {
    fn data_device_state(
        &mut self,
    ) -> &mut smithay::wayland::selection::data_device::DataDeviceState {
        &mut self.fht.data_device_state
    }
}

impl WaylandDndGrabHandler for State {
    fn dnd_requested<S: smithay::input::dnd::Source>(
        &mut self,
        source: S,
        icon: Option<WlSurface>,
        seat: smithay::input::Seat<Self>,
        serial: smithay::utils::Serial,
        type_: smithay::input::dnd::GrabType,
    ) {
        self.fht.dnd_icon = icon;
        match type_ {
            GrabType::Pointer => {
                let pointer = seat.get_pointer().unwrap();
                let start_data = pointer.grab_start_data().unwrap();
                let grab = DnDGrab::new_pointer(&self.fht.display_handle, start_data, source, seat);
                pointer.set_grab(self, grab, serial, Focus::Keep);
            }
            GrabType::Touch => {
                let touch = seat.get_touch().unwrap();
                let start_data = touch.grab_start_data().unwrap();
                let grab = DnDGrab::new_touch(&self.fht.display_handle, start_data, source, seat);
                touch.set_grab(self, grab, serial);
            }
        }

        self.fht.queue_redraw_all();
    }
}

impl DndGrabHandler for State {}

delegate_data_device!(State);
