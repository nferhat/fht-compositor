use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::idle_inhibit::IdleInhibitHandler;
use smithay::wayland::idle_notify::{IdleNotifierHandler, IdleNotifierState};
use smithay::{delegate_idle_inhibit, delegate_idle_notify};

use crate::state::{Fht, State};

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

impl Fht {
    /// Notify idle-notify clients about new activity that happened. This function is preferred to
    /// using [`IdleNotifierState::notify`], since it throttles the amount of times activity
    /// is notified of.
    pub fn idle_notify_activity(&mut self) {
        if !self.notified_idle_state {
            self.idle_notifier_state.notify_activity(&self.seat);
            self.notified_idle_state = true;
        }
    }
}

delegate_idle_notify!(State);
