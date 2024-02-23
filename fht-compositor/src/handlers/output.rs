use smithay::delegate_output;
use smithay::wayland::output::OutputHandler;

use crate::state::State;

impl OutputHandler for State {}

delegate_output!(State);
