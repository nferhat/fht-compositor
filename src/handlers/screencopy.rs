use crate::delegate_screencopy;
use crate::protocols::screencopy::{ScreencopyFrame, ScreencopyHandler};
use crate::state::State;

impl ScreencopyHandler for State {
    fn new_frame(&mut self, frame: ScreencopyFrame) {
        let Some(output_state) = self.fht.output_state.get_mut(frame.output()) else {
            warn!("wlr-screencopy frame with invalid output");
            return;
        };

        // A weird quirk with wlr-screencopy is that the clients decide of frame scheduling, not the
        // compositor, which causes some weird situations with our redraw loop.
        //
        // If the screencopy frame is requested with damage, we wait until the backend has damage
        // to submit. If the screencopy frame is requested without damage, we queue a redraw of the
        // output to satisfy the screencopy request on the next dispatch cycle.
        if !frame.with_damage() {
            // If we need damage, wait for the next render.
            output_state.redraw_state.queue();
        }

        output_state.pending_screencopies.push(frame);
    }
}

delegate_screencopy!(State);
