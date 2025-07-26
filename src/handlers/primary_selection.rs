use smithay::delegate_primary_selection;
use smithay::wayland::selection::primary_selection::PrimarySelectionHandler;

use crate::state::State;

impl PrimarySelectionHandler for State {
    fn primary_selection_state(
        &mut self,
    ) -> &mut smithay::wayland::selection::primary_selection::PrimarySelectionState {
        &mut self.fht.primary_selection_state
    }
}

delegate_primary_selection!(State);
