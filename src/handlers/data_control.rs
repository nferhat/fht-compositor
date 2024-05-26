use smithay::delegate_data_control;
use smithay::wayland::selection::wlr_data_control::DataControlHandler;

use crate::state::State;

impl DataControlHandler for State {
    fn data_control_state(
        &self,
    ) -> &smithay::wayland::selection::wlr_data_control::DataControlState {
        &self.fht.data_control_state
    }
}

delegate_data_control!(State);
