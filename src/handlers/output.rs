use smithay::delegate_output;
use smithay::wayland::output::OutputHandler;

use crate::protocols::ext_workspace;
use crate::state::State;

impl OutputHandler for State {
    fn output_bound(
        &mut self,
        output: smithay::output::Output,
        wl_output: smithay::reexports::wayland_server::protocol::wl_output::WlOutput,
    ) {
        // When the output is bound, we need to expose/bind them for the protocols to know about.
        // For example this makes workspace group enter this output.
        ext_workspace::workspace_group_enter(
            &mut self.fht.ext_workspace_manager_state,
            &output,
            &wl_output,
        );
    }
}

delegate_output!(State);
