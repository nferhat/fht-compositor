use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::Color32F;
use smithay::delegate_session_lock;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::utils::Point;
use smithay::wayland::compositor::{send_surface_state, with_states};
use smithay::wayland::fractional_scale::with_fractional_scale;
use smithay::wayland::session_lock::{self, LockSurface, SessionLockHandler};

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
        self.fht.lock_state = LockState::Pending(locker);
    }

    fn unlock(&mut self) {
        self.fht.lock_state = LockState::Unlocked;
        // "Unlock" all the outputs
        let outputs = self.fht.space.outputs().cloned().collect::<Vec<_>>();
        for output in &outputs {
            let output_state = self.fht.output_state.get_mut(output).unwrap();
            output_state.lock_backdrop = None;
            let _ = output_state.lock_surface.take();
        }
        // Reset focus
        self.update_keyboard_focus();
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

        let output_state = self.fht.output_state.get_mut(&output).unwrap();
        output_state.lock_surface = Some(lock_surface.clone());
        output_state.redraw_state.queue();

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
                Kind::Unspecified,
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
