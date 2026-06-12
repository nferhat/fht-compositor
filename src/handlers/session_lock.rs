use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::Color32F;
use smithay::delegate_session_lock;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::Point;
use smithay::wayland::compositor::{send_surface_state, with_states};
use smithay::wayland::fractional_scale::with_fractional_scale;
use smithay::wayland::session_lock::{self, LockSurface, SessionLockHandler, SessionLocker};

use crate::output::OutputExt;
use crate::renderer::FhtRenderer;
use crate::state::{Fht, State};

crate::fht_render_elements! {
    SessionLockRenderElement<R> => {
        ClearBackdrop = SolidColorRenderElement,
        LockSurface = WaylandSurfaceRenderElement<R>,
    }
}

const LOCKED_OUTPUT_BACKDROP_COLOR: Color32F = Color32F::new(0.05, 0.05, 0.05, 1.0);

impl SessionLockHandler for State {
    fn lock_state(&mut self) -> &mut session_lock::SessionLockManagerState {
        &mut self.fht.session_lock_manager_state
    }

    fn lock(&mut self, locker: session_lock::SessionLocker) {
        if let LockState::Locked(lock) = &self.fht.lock_state {
            // We are already locked:
            // 1. If the previous lock is still working, don't handle the new one. This could be an
            //    attack trying to replace the existing screenlock with one that accepts anything
            // 2. If the previous lock is dead, allow the new lock. This can be used as a fallback
            //    mechanism, for example this happened quite a lot when testing my lockscreen with
            //    quickshell.
            if lock.is_alive() {
                info!("Ignoring new lock request as session is already locked");
                return;
            }

            info!("locking session with new session lock");
            let lock = locker.ext_session_lock().clone();
            locker.lock();
            self.fht.lock_state = LockState::Locked(lock);

            return;
        }

        info!("locking session");
        // We are not locked already. We first need to wait for the lock client to create surfaces
        // for all the outputs, so that when we confirm the lock, all outputs are covered
        // properly.
        // Whenever the lock client commits a surface, we try to pass into the locked state.
        self.fht.lock_state = LockState::WaitingForSurfaces(locker);

        self.fht.queue_redraw_all();
    }

    fn unlock(&mut self) {
        self.fht.lock_state = LockState::Unlocked;
        // "Unlock" all the outputs
        let outputs = self.fht.space.outputs().cloned().collect::<Vec<_>>();
        for output in &outputs {
            let output_state = self.fht.output_state.get_mut(output).unwrap();
            let _ = output_state.lock_backdrop.take();
            let _ = output_state.lock_surface.take();
        }

        self.update_keyboard_focus();
        self.fht.queue_redraw_all();
    }

    fn new_surface(&mut self, lock_surface: LockSurface, wl_output: WlOutput) {
        trace!(id = %lock_surface.wl_surface().id(), "new lock surface");
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

        let output_state = self.fht.output_state.get_mut(&output).unwrap();
        output_state.lock_surface = Some(lock_surface.clone());
        output_state.redraw_state.queue();

        if output == *self.fht.space.active_output() {
            // Focus the newly placed lock surface.
            self.set_keyboard_focus(Some(lock_surface.wl_surface().clone()));
        }
    }
}

delegate_session_lock!(State);

impl Fht {
    pub fn is_locked(&self) -> bool {
        matches!(
            &self.lock_state,
            LockState::Locked(_) | LockState::Locking(_)
        )
    }

    pub fn try_continue_locking(&mut self) {
        if !matches!(self.lock_state, LockState::WaitingForSurfaces(_)) {
            return;
        }

        let has_surfaces = self
            .output_state
            .values()
            .all(|state| state.lock_surface.is_some());
        if !has_surfaces {
            trace!("not locking session, waiting for other lock surfaces");
            // still didn't map everything.
            return;
        }

        // Even though we have mapped surfaces, we still need to render a single frame with a clear
        // color backdrop. This is done in order to not leak session contents.
        let LockState::WaitingForSurfaces(locker) = std::mem::take(&mut self.lock_state) else {
            unreachable!()
        };

        trace!("starting lock procedure");
        self.lock_state = LockState::Locking(locker);
        // Initiate the redrawing with the lock backdrop
        self.queue_redraw_all();
    }

    pub fn session_lock_elements<R: FhtRenderer>(
        &mut self,
        renderer: &mut R,
        output: &Output,
    ) -> Vec<SessionLockRenderElement<R>> {
        let scale = output.current_scale().integer_scale() as f64;
        let mut elements = vec![];
        if !self.is_locked() {
            return elements;
        }

        let output_state = self.output_state.get_mut(output).unwrap();

        if let Some(lock_surface) = output_state.lock_surface.as_ref() {
            elements.extend(render_elements_from_surface_tree(
                renderer,
                lock_surface.wl_surface(),
                Point::default(),
                scale,
                1.0,
                // a lock surface is going to cover the entire screen, might aswell try to scan it
                // out if its possible, though it might be first placed on the primary plane.
                Kind::ScanoutCandidate,
            ));
        }

        // We still render a black drop to not show desktop content
        let solid_buffer = output_state.lock_backdrop.get_or_insert_with(|| {
            SolidColorBuffer::new(output.geometry().size, LOCKED_OUTPUT_BACKDROP_COLOR)
        });

        elements.push(
            SolidColorRenderElement::from_buffer(
                &*solid_buffer,
                Point::default(),
                scale,
                1.0,
                Kind::Unspecified,
            )
            .into(),
        );

        elements
    }
}

/// The locking state of the compositor.
#[derive(Default, Debug)]
pub enum LockState {
    /// The compositor is unlocked and displays content as usual.
    #[default]
    Unlocked,
    /// We are waiting for all lock surfaces to appear.
    WaitingForSurfaces(SessionLocker),
    /// The session is locking.
    Locking(SessionLocker),
    /// The session is locked.
    Locked(ExtSessionLockV1),
}
