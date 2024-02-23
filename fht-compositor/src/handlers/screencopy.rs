use std::cell::RefCell;
use std::time::Duration;

use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;

use crate::delegate_screencopy_manager;
use crate::protocols::screencopy::frame::Screencopy;
use crate::protocols::screencopy::ScreencopyHandler;
use crate::state::State;

pub type PendingScreencopy = RefCell<Option<Screencopy>>;

impl ScreencopyHandler for State {
    fn output(&mut self, output: &WlOutput) -> &Output {
        self.fht.outputs().find(|o| o.owns(output)).unwrap()
    }

    fn frame(&mut self, frame: Screencopy) {
        let output = frame.output.clone();
        let pending_screencopies: &PendingScreencopy =
            output.user_data().get_or_insert(|| RefCell::new(None));
        *pending_screencopies.borrow_mut() = Some(frame);
        if let Err(err) =
            self.backend
                .udev()
                .schedule_render(&output, Duration::ZERO, &self.fht.loop_handle)
        {
            warn!(?err, "Failed to schedule screencopy render!");
        };
    }
}

delegate_screencopy_manager!(State);
