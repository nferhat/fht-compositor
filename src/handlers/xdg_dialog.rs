use smithay::delegate_xdg_dialog;
use smithay::wayland::shell::xdg::dialog::XdgDialogHandler;
use smithay::wayland::shell::xdg::ToplevelSurface;

use crate::state::State;

impl XdgDialogHandler for State {
    fn modal_changed(&mut self, _toplevel: ToplevelSurface, _is_modal: bool) {
        // NOTE: We don't do any handling here since smithay will automatically set is_modal in
        // toplevel WlSurface user-data to true, so it will be handled during commit.
    }
}

delegate_xdg_dialog!(State);
