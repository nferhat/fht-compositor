use std::time::Duration;

use smithay::reexports::rustix::time::{clock_gettime, ClockId};

use crate::config::Easing;

/// A trait representing any kind of animation for the compositor needs.
pub struct Animation {
    start: f64,
    end: f64,
    easing: Easing,
    started_at: Duration, // TODO: Use an instant?
    current_time: Duration,
    duration: Duration,
}

impl Animation {
    /// Creates a new animation with given parameters.
    ///
    /// This should be used wisely.
    pub fn new(start: f64, end: f64, easing: Easing, duration: Duration) -> Self {
        assert!(
            !(start == end),
            "Tried to create an animation with the same start and end!"
        );

        // This is basically the same as smithay's clock struct
        let kernel_timespec = clock_gettime(ClockId::Monotonic);
        let started_at = Duration::new(
            kernel_timespec.tv_sec as u64,
            kernel_timespec.tv_nsec as u32,
        );

        Self {
            start,
            end,
            easing,
            started_at,
            current_time: started_at,
            duration,
        }
    }

    /// Set the current time of the animation.
    pub fn set_current_time(&mut self, new_current_time: Duration) {
        self.current_time = new_current_time;
    }

    /// Check whether the animation is finished or not.
    ///
    /// Basically checks the time.
    pub fn is_finished(&self) -> bool {
        self.current_time >= self.started_at + self.duration
    }

    /// Get the value at the current time
    pub fn value(&self) -> f64 {
        let passed = (self.current_time - self.started_at).as_secs_f64();
        let total = self.duration.as_secs_f64();
        let x = (passed / total).clamp(0., 1.);
        self.easing.y(x) * (self.end - self.start) + self.start
    }
}
