use keyframe::EasingFunction;
use serde::{Deserialize, Serialize};

pub mod cubic;
pub mod spring;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum AnimationCurve {
    /// Use a preset easing provided by [`keyframe`]
    Simple(Easing),
    /// Use a spring-based animation.
    Spring(spring::Animation),
    /// Use a custom cubic animation with two control points:
    Cubic(cubic::Animation),
}

impl Default for AnimationCurve {
    fn default() -> Self {
        Self::Simple(Easing::default())
    }
}

/// Wrapper enum including all the easings [`keyframe`] provides.
#[derive(Default, Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Easing {
    EaseIn,
    EaseInCubic,
    EaseInOut,
    EaseInOutCubic,
    EaseInOutQuart,
    EaseInOutQuint,
    EaseInQuad,
    EaseInQuart,
    EaseInQuint,
    EaseOut,
    EaseOutCubic,
    EaseOutQuad,
    EaseOutQuart,
    EaseOutQuint,
    #[default]
    Linear,
}

impl Easing {
    /// Get the Y value at a given X coordinate, assuming that x is included in [0.0, 1.0]
    pub fn y(&self, x: f64) -> f64 {
        match self {
            Self::EaseIn => keyframe::functions::EaseIn.y(x),
            Self::EaseInCubic => keyframe::functions::EaseInCubic.y(x),
            Self::EaseInOut => keyframe::functions::EaseInOut.y(x),
            Self::EaseInOutCubic => keyframe::functions::EaseInOutCubic.y(x),
            Self::EaseInOutQuart => keyframe::functions::EaseInOutQuart.y(x),
            Self::EaseInOutQuint => keyframe::functions::EaseInOutQuint.y(x),
            Self::EaseInQuad => keyframe::functions::EaseInQuad.y(x),
            Self::EaseInQuart => keyframe::functions::EaseInQuart.y(x),
            Self::EaseInQuint => keyframe::functions::EaseInQuint.y(x),
            Self::EaseOut => keyframe::functions::EaseOut.y(x),
            Self::EaseOutCubic => keyframe::functions::EaseOutCubic.y(x),
            Self::EaseOutQuad => keyframe::functions::EaseOutQuad.y(x),
            Self::EaseOutQuart => keyframe::functions::EaseOutQuart.y(x),
            Self::EaseOutQuint => keyframe::functions::EaseOutQuint.y(x),
            Self::Linear => keyframe::functions::Linear.y(x),
        }
    }
}
