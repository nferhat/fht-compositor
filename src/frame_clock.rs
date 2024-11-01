//! A frameclock for outputs.
//!
//! This implemenetation logic is inspired by Mutter's frame clock, `ClutterFrameClock`, with some
//! cases and checks removed since we are a much simpler compositor overall.
use std::num::NonZero;
use std::time::Duration;

use crate::utils::get_monotonic_time;

#[derive(Debug)]
pub struct FrameClock {
    // The number of nanoseconds between each presentation time.
    // This can be None for winit, since it does not have a set refresh time.
    refresh_interval_ns: Option<NonZero<u64>>,
    last_presentation_time: Option<Duration>,
}

impl FrameClock {
    pub fn new(refresh_interval: Option<Duration>) -> Self {
        Self {
            refresh_interval_ns: refresh_interval.map(|interval| {
                NonZero::new(interval.subsec_nanos().into())
                    .expect("refresh internal should never be zero")
            }),
            last_presentation_time: None,
        }
    }

    pub fn refresh_interval(&self) -> Option<Duration> {
        self.refresh_interval_ns
            .map(|r| Duration::from_nanos(r.get()))
    }

    /// Mark the latest presentation time `now` in the [`FrameClock`].
    pub fn present(&mut self, now: Duration) {
        self.last_presentation_time = Some(now);
    }

    /// Get the next presentation time of this clock
    pub fn next_presentation_time(&self) -> Duration {
        let now = get_monotonic_time();
        let Some(refresh_interval) = self
            .refresh_interval_ns
            .map(NonZero::get)
            .map(Duration::from_nanos)
        else {
            // Winit backend presents as soon as a redraw is done, since we don't have to wait for
            // a VBlank and instead just swap buffers
            return now;
        };
        let Some(last_presentation_time) = self.last_presentation_time else {
            // We did not present anything yet.
            return now;
        };

        // The common case is that the next presentation happens 1 refresh interval after the
        // last/previous presentation.
        //          |<last_presentation_time
        // |--------|----o---------|------>
        //           now>|         |<next_presentation_time
        let mut next_presentation_time = last_presentation_time + refresh_interval;
        // However, the last presentation could happen more than a frame ago, in the case of output
        // idling (IE. no damage on the output, so we do not redraw/present), or due to the GPU
        // being very busy (heavy render task)
        //
        // The following code adjusts next_presentation_time_us to be in the future,
        // but still aligned to display presentation times. Instead of
        if next_presentation_time < now {
            // Let's say we're just past next_presentation_time_us.
            //
            // First, we calculate current_phase_us, corresponding to the time since
            // the last integer multiple of the refresh interval passed after the last
            // presentation time. Subtracting this phase from now_us and adding a
            // refresh interval gets us the next possible presentation time after
            // now_us.
            //
            //     last_presentation_time_us
            //    /       next_presentation_time_us
            //   /       /   now_us
            //  /       /   /    new next_presentation_time_us
            // |-------|---o---|-------|--> possible presentation times
            //          \_/     \_____/
            //          /           \
            // current_phase_us      refresh_interval_us
            //

            let current_phase = Duration::from_nanos(
                ((now.as_nanos() - last_presentation_time.as_nanos()) % refresh_interval.as_nanos())
                    as u64, // FIXME: can overflow, but we dont care about it
            );
            next_presentation_time = now - current_phase + refresh_interval;
        }

        // time_since_last_next_presentation_time_us =
        //   next_presentation_time_us - last_presentation->next_presentation_time_us;
        // if (time_since_last_next_presentation_time_us > 0 &&
        //     time_since_last_next_presentation_time_us < (refresh_interval_us / 2))
        //   {
        //     next_presentation_time_us =
        //       frame_clock->next_presentation_time_us + refresh_interval_us;
        //   }

        next_presentation_time
    }
}
