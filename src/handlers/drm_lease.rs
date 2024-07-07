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
            .lease_state
            .as_mut()
            .unwrap()
    }

    fn lease_request(
        &mut self,
        node: DrmNode,
        request: DrmLeaseRequest,
    ) -> Result<DrmLeaseBuilder, LeaseRejected> {
        let backend = self
            .backend
            .udev()
            .devices
            .get(&node)
            .ok_or(LeaseRejected::default())?;

        let mut builder = DrmLeaseBuilder::new(&backend.drm);
        for conn in request.connectors {
            if let Some((_, crtc)) = backend
                .non_desktop_connectors
                .iter()
                .find(|(handle, _)| *handle == conn)
            {
                builder.add_connector(conn);
                builder.add_crtc(*crtc);
                let planes = backend
                    .drm
                    .planes(crtc)
                    .map_err(LeaseRejected::with_cause)?;

                let (primary_plane, primary_plane_claim) = planes
                    .primary
                    .iter()
                    .find_map(|plane| {
                        backend
                            .drm
                            .claim_plane(plane.handle, *crtc)
                            .map(|claim| (plane, claim))
                    })
                    .ok_or_else(LeaseRejected::default)?;
                builder.add_plane(primary_plane.handle, primary_plane_claim);
                if let Some((cursor, claim)) = planes.cursor.and_then(|plane| {
                    backend
                        .drm
                        .claim_plane(plane.handle, *crtc)
                        .map(|claim| (plane, claim))
                }) {
                    builder.add_plane(cursor.handle, claim);
                }
            } else {
                warn!(
                    ?conn,
                    "Lease requested for desktop connector, denying request"
                );
                return Err(LeaseRejected::default());
            }
        }

        Ok(builder)
    }

    fn new_active_lease(&mut self, node: DrmNode, lease: DrmLease) {
        let backend = self.backend.udev().devices.get_mut(&node).unwrap();
        backend.active_leases.push(lease);
    }

    fn lease_destroyed(&mut self, node: DrmNode, lease_id: u32) {
        let backend = self.backend.udev().devices.get_mut(&node).unwrap();
        backend.active_leases.retain(|l| l.id() != lease_id);
    }
}

delegate_drm_lease!(State);
