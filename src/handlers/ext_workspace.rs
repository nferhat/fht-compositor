use crate::delegate_ext_workspace;
use crate::protocols::ext_workspace::{ExtWorkspaceHandler, ExtWorkspaceManagerState};
use crate::space::WorkspaceId;
use crate::state::State;

impl ExtWorkspaceHandler for State {
    fn ext_workspace_manager_state(&mut self) -> &mut ExtWorkspaceManagerState {
        &mut self.fht.ext_workspace_manager_state
    }

    fn activate_workspace(&mut self, id: WorkspaceId) {
        if !self.fht.space.activate_workspace(id, true) {
            warn!(?id, "ext-workspace tried to activate invalid workspace");
        }
    }
}

delegate_ext_workspace!(State);
