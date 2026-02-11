use smithay::delegate_output;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::wayland::output::OutputHandler;

use crate::protocols::ext_workspace;
use crate::state::State;

impl OutputHandler for State {
    fn output_bound(&mut self, output: Output, wl_output: WlOutput) {
        ext_workspace::on_output_bound(self, &output, &wl_output);
    }
}

delegate_output!(State);
