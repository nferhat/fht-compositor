use smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::selection::data_device::{ClientDndGrabHandler, ServerDndGrabHandler};

use crate::state::State;

impl ClientDndGrabHandler for State {
    fn started(
        &mut self,
        _source: Option<WlDataSource>,
        icon: Option<WlSurface>,
        _seat: smithay::input::Seat<Self>,
    ) {
        self.fht.dnd_icon = icon;
    }

    fn dropped(
        &mut self,
        _target: Option<WlSurface>,
        _validated: bool,
        _seat: smithay::input::Seat<Self>,
    ) {
        self.fht.dnd_icon = None;
    }
}

impl ServerDndGrabHandler for State {
    fn send(
        &mut self,
        _mime_type: String,
        _fd: std::os::unix::prelude::OwnedFd,
        _seat: smithay::input::Seat<Self>,
    ) {
        unreachable!("We don't support server-side grabs");
    }
}
