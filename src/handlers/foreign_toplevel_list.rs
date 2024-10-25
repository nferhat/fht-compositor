use smithay::delegate_foreign_toplevel_list;
use smithay::wayland::foreign_toplevel_list::ForeignToplevelListHandler;

use crate::state::State;

impl ForeignToplevelListHandler for State {
    fn foreign_toplevel_list_state(
        &mut self,
    ) -> &mut smithay::wayland::foreign_toplevel_list::ForeignToplevelListState {
        &mut self.fht.foreign_toplevel_list_state
    }
}

delegate_foreign_toplevel_list!(State);
