use crate::delegate_screencopy;
use crate::protocols::screencopy::{Screencopy, ScreencopyHandler};
use crate::state::State;

impl ScreencopyHandler for State {
    fn frame(&mut self, frame: Screencopy) {
        let Some(output_state) = self.fht.output_state.get_mut(frame.output()) else {
            warn!("wlr-screencopy frame with invalid output");
            return;
        };

        if !frame.with_damage() {
            // If we need damage, wait for the next render.
            output_state.redraw_state.queue();
        }

        output_state.pending_screencopy = Some(frame);
    }
}

delegate_screencopy!(State);
