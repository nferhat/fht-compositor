use smithay::delegate_xdg_foreign;
use smithay::wayland::xdg_foreign::XdgForeignHandler;

use crate::state::State;

impl XdgForeignHandler for State {
    fn xdg_foreign_state(&mut self) -> &mut smithay::wayland::xdg_foreign::XdgForeignState {
        &mut self.fht.xdg_foreign_state
    }
}

delegate_xdg_foreign!(State);
