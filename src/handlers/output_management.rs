use std::collections::HashMap;

use crate::delegate_output_management;
use crate::protocols::output_management::{OutputManagementHandler, OutputManagementManagerState};
use crate::state::State;

impl OutputManagementHandler for State {
    fn output_management_manager_state(&mut self) -> &mut OutputManagementManagerState {
        &mut self.fht.output_management_manager_state
    }

    fn apply_configuration(
        &mut self,
        config: HashMap<String, fht_compositor_config::Output>,
    ) -> bool {
        // FIXME: Disable outputs.
        let changed = self.fht.config.outputs != config;
        self.fht.config.outputs = config;
        self.fht.has_transient_output_changes = changed;
        self.fht.reload_output_config();
        // FIXME: Actually check whether the output configs have been applied.
        // This is more complicated since we need to ask the backend whether we actually applied
        // everything properly
        true
    }
}

delegate_output_management!(State);
