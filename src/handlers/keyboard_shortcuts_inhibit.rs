use smithay::delegate_keyboard_shortcuts_inhibit;
use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitHandler;

use crate::state::State;

impl KeyboardShortcutsInhibitHandler for State {
    fn keyboard_shortcuts_inhibit_state(
        &mut self,
    ) -> &mut smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState {
        &mut self.fht.keyboard_shortcuts_inhibit_state
    }

    fn new_inhibitor(
        &mut self,
        inhibitor: smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitor,
    ) {
        // Just allow it
        // TODO: Maybe filter?
        inhibitor.activate();
    }
}

delegate_keyboard_shortcuts_inhibit!(State);
