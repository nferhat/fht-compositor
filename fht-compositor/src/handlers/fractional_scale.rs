use smithay::delegate_fractional_scale;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::fractional_scale::FractionalScaleHandler;

use crate::state::State;

impl FractionalScaleHandler for State {
    fn new_fractional_scale(&mut self, _surface: WlSurface) {
        // TODO: Initiate fractional scale
    }
}

delegate_fractional_scale!(State);
