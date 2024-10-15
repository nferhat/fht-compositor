use smithay::{
    delegate_session_lock,
    output::Output,
    reexports::wayland_server::protocol::wl_output::WlOutput,
    wayland::{
        compositor::{send_surface_state, with_states},
        fractional_scale::with_fractional_scale,
        session_lock::{self, LockSurface, SessionLockHandler},
    },
};

use crate::{
    state::{Fht, OutputState, State},
    utils::output::OutputExt,
};

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
    }

    fn new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        let Some(output) = Output::from_resource(&wl_output) else {
            return;
        };

        // Configure our surface for the output
        let output_size = output.geometry().size;
        surface.with_pending_state(|state| {
            state.size = Some((output_size.w as u32, output_size.h as u32).into());
        });
        let scale = output.current_scale();
        let transform = output.current_transform();
        let wl_surface = surface.wl_surface();
        with_states(wl_surface, |data| {
            send_surface_state(wl_surface, data, scale.integer_scale(), transform);
            with_fractional_scale(data, |fractional| {
                fractional.set_preferred_scale(scale.fractional_scale());
            });
        });

        surface.send_configure();

        OutputState::get(&output).lock_surface = Some(surface);
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
    /// The compositor has received a lock request and is in the process of drawing a black backdrop
    /// Over all the [`Output`]s
    Pending(session_lock::SessionLocker),
    /// The compositor is fully locked.
    Locked,
}
