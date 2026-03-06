use smithay::backend::drm::DrmNode;
use smithay::delegate_drm_lease;
use smithay::wayland::drm_lease::{
    DrmLease, DrmLeaseBuilder, DrmLeaseHandler, DrmLeaseRequest, DrmLeaseState, LeaseRejected,
};

use crate::state::State;

impl DrmLeaseHandler for State {
    fn drm_lease_state(&mut self, node: DrmNode) -> &mut DrmLeaseState {
        self.backend
            .udev()
            .devices
            .get_mut(&node)
            .unwrap()
            .lease_state()
            .unwrap()
    }

    fn lease_request(
        &mut self,
        node: DrmNode,
        request: DrmLeaseRequest,
    ) -> Result<DrmLeaseBuilder, LeaseRejected> {
        let device = self
            .backend
            .udev()
            .devices
            .get(&node)
            .ok_or(LeaseRejected::default())?;
        device.lease_request(request)
    }

    fn new_active_lease(&mut self, node: DrmNode, lease: DrmLease) {
        let backend = self.backend.udev().devices.get_mut(&node).unwrap();
        backend.add_active_lease(lease);
    }

    fn lease_destroyed(&mut self, node: DrmNode, lease_id: u32) {
        let device = self.backend.udev().devices.get_mut(&node).unwrap();
        device.remove_lease(lease_id);
    }
}

delegate_drm_lease!(State);
