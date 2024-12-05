use smithay::{
    delegate_idle_inhibit, delegate_idle_notify,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::{
        idle_inhibit::IdleInhibitHandler,
        idle_notify::{IdleNotifierHandler, IdleNotifierState},
    },
};

use crate::state::State;

impl IdleInhibitHandler for State {
    fn inhibit(&mut self, surface: WlSurface) {
        self.fht.idle_inhibiting_surfaces.push(surface);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.fht.idle_inhibiting_surfaces.retain(|s| *s != surface);
    }
}

delegate_idle_inhibit!(State);

impl IdleNotifierHandler for State {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<State> {
        &mut self.fht.idle_notifier_state
    }
}

delegate_idle_notify!(State);
