use crate::delegate_hyprland_global_shortcuts;
use crate::protocols::hyprland_global_shortcuts::{
    HyprlandGlobalShortcutsHandler, HyprlandGlobalShortcutsState, ShortcutData,
};
use crate::state::State;

impl HyprlandGlobalShortcutsHandler for State {
    fn hyprland_global_shortcuts_state(&mut self) -> &mut HyprlandGlobalShortcutsState {
        &mut self.fht.hyprland_global_shortcuts_state
    }

    fn new_shortcut(&mut self, data: ShortcutData) {
        debug!(
            app_id = %data.app_id,
            id = %data.id,
            description = %data.description,
            "Registered global shortcut"
        );
    }

    fn shortcut_destroyed(&mut self, app_id: String, id: String) {
        debug!(%app_id, %id, "Global shortcut destroyed");
    }
}

delegate_hyprland_global_shortcuts!(State);
