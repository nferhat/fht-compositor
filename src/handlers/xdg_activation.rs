use smithay::delegate_xdg_activation;
use smithay::input::Seat;
use smithay::reexports::wayland_server::protocol::wl_surface;
use smithay::wayland::xdg_activation::{self, XdgActivationHandler};

use crate::state::State;
use crate::utils::output::OutputExt;
use crate::utils::RectCenterExt;

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
            // First part: focus the window inside the workspace.
            let Some((window, workspace)) = self.fht.find_window_and_workspace_mut(&surface) else {
                return;
            };
            workspace.focus_window(&window, true);

            // Second part: focus the workspace of the workspace set.
            let (window, output) = self.fht.find_window_and_output(&surface).unwrap();
            let (window, output) = (window.clone(), output.clone());
            // This is quite tricky, since we need to find *the* workspace with this window.
            //
            // If we care about performance we'd use workspace ids and stuff, but this is just not
            // it. But this is for this little task unnecessary, just use Iter::position.
            let wset = self.fht.wset_mut_for(&output);
            let workspace_idx = wset
                .workspaces()
                .position(|ws| ws.has_window(&window))
                .unwrap();
            wset.set_active_idx(workspace_idx, true);

            // Finally, focus the output.
            // This code is copied from src/input/actions.rs, for Focus{Next,Previous}Output
            // actions
            if self
                .fht
                .focus_state
                .output
                .as_ref()
                .is_some_and(|o| *o != output)
            {
                if self.fht.config.general.cursor_warps {
                    let center = output.geometry().center();
                    self.move_pointer(center.to_f64());
                }
                let _ = self.fht.focus_state.output.replace(output);
            }
        }
    }
}

delegate_xdg_activation!(State);
