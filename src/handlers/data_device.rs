use smithay::delegate_data_device;
use smithay::wayland::selection::data_device::DataDeviceHandler;

use crate::state::State;

impl DataDeviceHandler for State {
    fn data_device_state(
        &mut self,
    ) -> &mut smithay::wayland::selection::data_device::DataDeviceState {
        &mut self.fht.data_device_state
    }
}

delegate_data_device!(State);
