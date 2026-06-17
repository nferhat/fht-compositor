//! A frameclock for outputs.
//!
//! This implemenetation logic is inspired by Niri's frame clock,
use std::num::NonZero;
use std::time::Duration;

use crate::utils::get_monotonic_time;

#[derive(Debug)]
pub struct FrameClock {
    // The number of nanoseconds between each presentation time.
    // This can be None for winit, since it does not have a set refresh time.
    refresh_interval_ns: Option<NonZero<u64>>,
    last_presentation_time: Option<Duration>,
    vrr: bool,
}

impl FrameClock {
    pub fn new(refresh_interval: Option<Duration>, vrr: bool) -> Self {
        Self {
            refresh_interval_ns: refresh_interval.map(|interval| {
                NonZero::new(interval.subsec_nanos().into())
                    .expect("refresh internal should never be zero")
            }),
            last_presentation_time: None,
            vrr,
        }
    }

    #[allow(unused)] // no used in winit
    pub fn refresh_interval(&self) -> Option<Duration> {
        self.refresh_interval_ns
            .map(|r| Duration::from_nanos(r.get()))
    }

    #[allow(unused)] // no used in udev
    pub fn set_vrr(&mut self, vrr: bool) {
        if self.vrr == vrr {
            return;
        }

        self.vrr = vrr;
        self.last_presentation_time = None;
    }

    /// Mark the latest presentation time `now` in the [`FrameClock`].
    pub fn present(&mut self, now: Duration) {
        self.last_presentation_time = Some(now);
    }

    /// Get the next presentation time of this clock
    pub fn next_presentation_time(&self) -> Duration {
        let mut now = get_monotonic_time();
        let Some(refresh_interval_ns) = self.refresh_interval_ns.map(NonZero::get) else {
            // Winit backend presents as soon as a redraw is done, since we don't have to wait for
            // a VBlank and instead just swap buffers
            return now;
        };
        let Some(last_presentation_time) = self.last_presentation_time else {
            // We did not present anything yet.
            return now;
        };

        if now <= last_presentation_time {
            // Early vlank event, shift for next redraw cycle
            now += Duration::from_nanos(refresh_interval_ns);
        }

        let since_last = now - last_presentation_time;
        let since_last_ns =
            since_last.as_secs() * 1_000_000_000 + u64::from(since_last.subsec_nanos());
        let to_next_ns = (since_last_ns / refresh_interval_ns + 1) * refresh_interval_ns;

        // If VRR is enabled and more than one frame passed since last presentation, assume that we
        // can present immediately.
        if self.vrr && to_next_ns > refresh_interval_ns {
            now
        } else {
            last_presentation_time + Duration::from_nanos(to_next_ns)
        }
    }
}
