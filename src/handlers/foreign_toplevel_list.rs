use smithay::wayland::foreign_toplevel_list::ForeignToplevelListHandler;
use smithay::{
    delegate_foreign_toplevel_list, wayland::foreign_toplevel_list::ForeignToplevelListState,
};

use crate::{
    state::{Fht, State},
    window::Window,
};

impl ForeignToplevelListHandler for State {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.fht.foreign_toplevel_list_state
    }
}

impl Fht {
    /// Adversite a new [`Window`] with the ext-foreignt-toplevel-v1 protocol.
    ///
    /// This creates the toplevel handle and stores it inside the [`Window`].
    pub fn adversite_new_foreign_window(&mut self, window: &Window) {
        if window.foreign_toplevel_handle().is_some() {
            warn!(window = ?window.id(), "Tried to adversite window to ext-foreign-toplevel-v1 twice");
            return;
        }

        // NOTE: smithay handles the identifier part for us.
        let handle = self
            .foreign_toplevel_list_state
            .new_toplevel::<State>(window.title().unwrap(), window.app_id().unwrap_or_default());

        // send all initial data.
        handle.send_title(window.title().as_ref().unwrap());
        handle.send_app_id(window.app_id().as_ref().unwrap());
        handle.send_done();

        window.set_foreign_toplevel_handle(handle);
    }

    /// De-adversite a [`Window`] with the ext-foreignt-toplevel-v1 protocol.
    pub fn close_foreign_handle(&mut self, window: &Window) {
        let Some(handle) = window.take_foreign_toplevel_handle() else {
            // this can happen, for example unmapped window gets removed.
            return;
        };

        self.foreign_toplevel_list_state.remove_toplevel(&handle);
    }

    /// Send new window details for all ext-toplevel-foreign-list instances.
    pub fn send_foreign_window_details(&mut self, window: &Window) {
        if let Some(handle) = window.foreign_toplevel_handle() {
            handle.send_title(window.title().as_ref().unwrap());
            handle.send_app_id(window.app_id().as_ref().unwrap());
        } else {
            // it was not adversited before, this should be done on-map
            // this shoud not happen though.
            warn!(window = ?window.id(), "Tried updating foreign toplevel handle details for window without one");
            self.adversite_new_foreign_window(window);
        }
    }
}

delegate_foreign_toplevel_list!(State);
