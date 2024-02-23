use smithay::delegate_shm;
use smithay::wayland::shm::ShmHandler;

use crate::state::State;

impl ShmHandler for State {
    fn shm_state(&self) -> &smithay::wayland::shm::ShmState {
        &self.fht.shm_state
    }
}

delegate_shm!(State);
