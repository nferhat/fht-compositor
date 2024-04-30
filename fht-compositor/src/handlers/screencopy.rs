use crate::delegate_screencopy;
use crate::protocols::screencopy::{Screencopy, ScreencopyHandler};
use crate::state::{OutputState, State};

impl ScreencopyHandler for State {
    fn frame(&mut self, frame: Screencopy) {
        // With wlr-screencopy, its up to the clients to manage frame timings, and not the
        // compositor. We can't render at any time, so we just set this frame pending and submit to
        // it by the next render.

        let output = frame.output().clone();
        let mut state = OutputState::get(&output);

        if !frame.with_damage() {
            // If we need damage, wait for the next render.
            state.render_state.queue();
        }

        state.pending_screencopy = Some(frame);
    }
}

delegate_screencopy!(State);
