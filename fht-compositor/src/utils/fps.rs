// Thank you cosmic-comp
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Fps {
    pending_frame: Option<PendingFrame>,
    pub frames: VecDeque<Frame>,
}

#[derive(Debug)]
struct PendingFrame {
    start: Instant,
    duration_elements: Option<Duration>,
    duration_render: Option<Duration>,
    duration_displayed: Option<Duration>,
}

#[derive(Debug)]
pub struct Frame {
    pub start: Instant,
    pub duration_elements: Duration,
    pub duration_render: Duration,
    pub duration_displayed: Duration,
}

impl Frame {
    fn render_time(&self) -> Duration {
        self.duration_elements + self.duration_render
    }
}

impl From<PendingFrame> for Frame {
    fn from(pending: PendingFrame) -> Self {
        Frame {
            start: pending.start,
            duration_elements: pending.duration_elements.unwrap_or(Duration::ZERO),
            duration_render: pending.duration_render.unwrap_or(Duration::ZERO),
            duration_displayed: pending.duration_displayed.unwrap_or(Duration::ZERO),
        }
    }
}

impl Fps {
    const WINDOW_SIZE: usize = 360;

    pub fn start(&mut self) {
        self.pending_frame = Some(PendingFrame {
            start: Instant::now(),
            duration_elements: None,
            duration_render: None,
            duration_displayed: None,
        });
    }

    pub fn elements(&mut self) {
        if let Some(frame) = self.pending_frame.as_mut() {
            frame.duration_elements = Some(Instant::now().duration_since(frame.start));
        }
    }

    pub fn render(&mut self) {
        if let Some(frame) = self.pending_frame.as_mut() {
            frame.duration_render = Some(
                Instant::now().duration_since(frame.start)
                    - frame.duration_elements.clone().unwrap_or(Duration::ZERO),
            );
        }
    }

    pub fn displayed(&mut self) {
        if let Some(mut frame) = self.pending_frame.take() {
            frame.duration_displayed = Some(
                Instant::now().duration_since(frame.start)
                    - frame.duration_elements.clone().unwrap_or(Duration::ZERO)
                    - frame.duration_render.clone().unwrap_or(Duration::ZERO)
            );

            self.frames.push_back(frame.into());
            while self.frames.len() > Fps::WINDOW_SIZE {
                self.frames.pop_front();
            }
        }
    }

    pub fn max_frametime(&self) -> Duration {
        self.frames
            .iter()
            .map(Frame::render_time)
            .max()
            .unwrap_or(Duration::ZERO)
    }

    pub fn min_frametime(&self) -> Duration {
        self.frames
            .iter()
            .map(Frame::render_time)
            .min()
            .unwrap_or(Duration::ZERO)
    }

    pub fn avg_frametime(&self) -> Duration {
        if self.frames.is_empty() {
            return Duration::ZERO;
        }
        self.frames.iter().map(Frame::render_time).sum::<Duration>() / (self.frames.len() as u32)
    }

    pub fn avg_rendertime(&self, window: usize) -> Duration {
        self.frames
            .iter()
            .take(window)
            .map(|f| f.render_time())
            .sum::<Duration>()
            / window as u32
    }

    pub fn avg_fps(&self) -> f64 {
        if self.frames.is_empty() {
            return 0.0;
        }
        let secs = match (self.frames.front(), self.frames.back()) {
            (Some(Frame { start, .. }), Some(end_frame)) => {
                end_frame.start.duration_since(*start) + end_frame.render_time()
            }
            _ => Duration::ZERO,
        }
        .as_secs_f64();
        1.0 / (secs / self.frames.len() as f64)
    }
}

impl Fps {
    pub fn new() -> Fps {
        Fps {
            pending_frame: None,
            frames: VecDeque::with_capacity(Fps::WINDOW_SIZE + 1),
        }
    }
}
