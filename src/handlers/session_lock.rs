use smithay::delegate_session_lock;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::wayland::compositor::{send_surface_state, with_states};
use smithay::wayland::fractional_scale::with_fractional_scale;
use smithay::wayland::session_lock::{self, LockSurface, SessionLockHandler};

use crate::state::{Fht, OutputState, State};
use crate::utils::output::OutputExt;

impl SessionLockHandler for State {
    fn lock_state(&mut self) -> &mut session_lock::SessionLockManagerState {
        &mut self.fht.session_lock_manager_state
    }

    fn lock(&mut self, locker: session_lock::SessionLocker) {
        self.fht.lock_state = LockState::Pending(locker);
    }

    fn unlock(&mut self) {
        self.fht.lock_state = LockState::Unlocked;
        // "Unlock" all the outputs
        for output in self.fht.space.outputs() {
            let mut output_state = OutputState::get(output);
            output_state.has_lock_backdrop = false;
            let _ = output_state.lock_surface.take();
        }
        // Reset focus
        let active_window = self.fht.space.active_window();
        self.set_keyboard_focus(active_window);
    }

    fn new_surface(&mut self, lock_surface: LockSurface, wl_output: WlOutput) {
        let Some(output) = Output::from_resource(&wl_output) else {
            return;
        };

        // Configure our surface for the output
        let output_size = output.geometry().size;
        lock_surface.with_pending_state(|state| {
            state.size = Some((output_size.w as u32, output_size.h as u32).into());
        });
        let scale = output.current_scale();
        let transform = output.current_transform();
        let wl_surface = lock_surface.wl_surface();
        with_states(wl_surface, |data| {
            send_surface_state(wl_surface, data, scale.integer_scale(), transform);
            with_fractional_scale(data, |fractional| {
                fractional.set_preferred_scale(scale.fractional_scale());
            });
        });

        lock_surface.send_configure();

        OutputState::get(&output).lock_surface = Some(lock_surface.clone());
        if output == *self.fht.space.active_output() {
            // Focus the newly placed lock surface.
            self.set_keyboard_focus(Some(lock_surface));
        }
    }
}

delegate_session_lock!(State);

impl Fht {
    pub fn is_locked(&self) -> bool {
        matches!(&self.lock_state, LockState::Locked | LockState::Pending(_))
    }
}

/// The locking state of the compositor.
///
/// Needed in order to notify the session lock confirmation that we drew a black backdrop over all
/// the outputs of the compositor.
#[derive(Default, Debug)]
pub enum LockState {
    /// The compositor is unlocked and displays content as usual.
    #[default]
    Unlocked,
    /// The compositor has received a lock request and is in the process of drawing a black
    /// backdrop Over all the [`Output`]s
    Pending(session_lock::SessionLocker),
    /// The compositor is fully locked.
    Locked,
}
