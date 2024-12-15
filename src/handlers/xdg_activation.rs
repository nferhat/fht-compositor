use smithay::delegate_xdg_activation;
use smithay::input::Seat;
use smithay::reexports::wayland_server::protocol::wl_surface;
use smithay::wayland::xdg_activation::{self, XdgActivationHandler};

use crate::state::State;

pub const ACTIVATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut xdg_activation::XdgActivationState {
        &mut self.fht.xdg_activation_state
    }

    fn token_created(
        &mut self,
        _token: xdg_activation::XdgActivationToken,
        data: xdg_activation::XdgActivationTokenData,
    ) -> bool {
        if let Some((serial, seat)) = data.serial {
            Seat::from_resource(&seat).as_ref() == Some(&self.fht.seat)
                && self
                    .fht
                    .keyboard
                    .last_enter()
                    .map(|le| serial.is_no_older_than(&le))
                    .unwrap_or(false)
        } else {
            false
        }
    }

    fn request_activation(
        &mut self,
        _token: xdg_activation::XdgActivationToken,
        token_data: xdg_activation::XdgActivationTokenData,
        surface: wl_surface::WlSurface,
    ) {
        if token_data.timestamp.elapsed() < ACTIVATION_TIMEOUT {
            let mut output = None;
            if let Some((window, workspace)) = self.fht.space.find_window_and_workspace(&surface) {
                output = Some(workspace.output().clone());
                self.fht.space.activate_window(&window, true);
            }

            if let Some(ref output) = output {
                self.fht.queue_redraw(output);
            }
        }
    }
}

delegate_xdg_activation!(State);
