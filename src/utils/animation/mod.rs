pub mod curve;

use std::time::Duration;

use smithay::reexports::rustix::time::{clock_gettime, ClockId};
use smithay::utils::{Coordinate, Monotonic, Point, Time};

use self::curve::AnimationCurve;

pub trait Animatable:
    Sized
    + std::fmt::Debug
    + std::ops::Add<Output = Self>
    + std::ops::Sub<Output = Self>
    + Copy
    + PartialEq
{
    fn y(&self, x: f64) -> Self;
}

impl<Kind> Animatable for Point<f64, Kind> {
    fn y(&self, x: f64) -> Self {
        self.to_f64().upscale(x).into()
    }
}

impl<Kind> Animatable for Point<i32, Kind> {
    fn y(&self, x: f64) -> Self {
        self.to_f64().upscale(x).to_i32_round()
    }
}

impl Animatable for i32 {
    fn y(&self, x: f64) -> Self {
        (*self as f64).saturating_mul(x).round() as i32
    }
}

impl Animatable for f64 {
    fn y(&self, x: f64) -> Self {
        self.saturating_mul(x)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Animation<T = f64>
where
    T: Animatable,
{
    pub start: T,
    pub end: T,
    current_value: T,
    curve: AnimationCurve,
    started_at: Time<Monotonic>,
    current_time: Time<Monotonic>,
    duration: Duration,
}

impl<T: Animatable> Animation<T> {
    pub fn new(start: T, end: T, curve: AnimationCurve, mut duration: Duration) -> Option<Self> {
        if start == end {
            return None;
        }

        if duration.is_zero() {
            return None;
        }

        // This is basically the same as smithay's monotonic clock struct
        let kernel_timespec = clock_gettime(ClockId::Monotonic);
        let started_at = Duration::new(
            kernel_timespec.tv_sec as u64,
            kernel_timespec.tv_nsec as u32,
        )
        .into();

        // If we are using spring animations just ignore whatever the user puts for the duration.
        if let AnimationCurve::Spring(spring) = &curve {
            duration = spring.duration();
        }

        Some(Self {
            start,
            end,
            current_value: start,
            curve,
            started_at,
            current_time: started_at,
            duration,
        })
    }

    pub fn set_current_time(&mut self, new_current_time: Time<Monotonic>) {
        self.current_time = new_current_time;
        self.current_value = match &mut self.curve {
            AnimationCurve::Simple(easing) => {
                // keyframe's easing function take an x value between [0.0, 1.0], so normalize out
                // x value to these.
                let elapsed = Time::elapsed(&self.started_at, self.current_time).as_secs_f64();
                let total = self.duration.as_secs_f64();
                let x = (elapsed / total).clamp(0., 1.);
                let easing_x = easing.y(x);
                (self.end - self.start).y(easing_x) + self.start
            }
            AnimationCurve::Cubic(cubic) => {
                // Cubic animations also take in X between [0.0, 1.0] and outputs a progress in
                // [0.0, 1.0]
                let elapsed = Time::elapsed(&self.started_at, self.current_time).as_secs_f64();
                let total = self.duration.as_secs_f64();
                let x = (elapsed / total).clamp(0., 1.);
                let cubic_x = cubic.y(x);
                (self.end - self.start).y(cubic_x) + self.start
            }
            AnimationCurve::Spring(spring) => {
                let elapsed = Time::elapsed(&self.started_at, self.current_time).as_secs_f64();
                let x = spring.oscillate(elapsed);
                (self.end - self.start).y(x) + self.start
            }
        };
    }

    pub fn is_finished(&self) -> bool {
        Time::elapsed(&self.started_at, self.current_time) >= self.duration
    }

    pub fn value(&self) -> T {
        self.current_value
    }
}
