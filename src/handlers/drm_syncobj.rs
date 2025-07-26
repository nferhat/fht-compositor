use smithay::delegate_drm_syncobj;
use smithay::wayland::drm_syncobj::{DrmSyncobjHandler, DrmSyncobjState};

use crate::state::State;

impl DrmSyncobjHandler for State {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        let backend = self.backend.udev();
        backend.syncobj_state.as_mut()
    }
}

delegate_drm_syncobj!(State);
