use keyframe::EasingFunction;
use serde::{Deserialize, Serialize};

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

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct AnimationConfig {
    /// The animation for workspaces switches
    #[serde(default)]
    pub workspace_switch: WorkspaceSwitchAnimationConfig,

    /// The animation when opening and closing windows
    #[serde(default)]
    pub window_open_close: WindowOpenCloseAnimation,

    /// The animation when windows change their geometry
    #[serde(default)]
    pub window_geometry: WindowGeometryAnimation,
}

const fn default_workspace_switch_animation_duration() -> u64 {
    350
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSwitchAnimationConfig {
    /// What easing to use for the animation:
    #[serde(default)]
    pub easing: Easing,
    /// The duration of the animation, in milliseconds.
    #[serde(default = "default_workspace_switch_animation_duration")]
    pub duration: u64,
    /// The direction, whether to switch vertically of horizontally.
    #[serde(default)]
    pub direction: WorkspaceSwitchAnimationDirection,
}

impl Default for WorkspaceSwitchAnimationConfig {
    fn default() -> Self {
        Self {
            easing: Easing::default(),
            duration: 350,
            direction: WorkspaceSwitchAnimationDirection::Horizontal,
        }
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceSwitchAnimationDirection {
    #[default]
    Horizontal,
    Vertical,
}

const fn default_window_animation_duration() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowOpenCloseAnimation {
    /// What easing to use for the animation:
    #[serde(default)]
    pub easing: Easing,
    /// The duration of the animation, in milliseconds.
    #[serde(default = "default_window_animation_duration")]
    pub duration: u64,
}

impl Default for WindowOpenCloseAnimation {
    fn default() -> Self {
        Self {
            easing: Easing::default(),
            duration: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowGeometryAnimation {
    /// What easing to use for the animation:
    #[serde(default)]
    pub easing: Easing,
    /// The duration of the animation, in milliseconds.
    #[serde(default = "default_window_animation_duration")]
    pub duration: u64,
}

impl Default for WindowGeometryAnimation {
    fn default() -> Self {
        Self {
            easing: Easing::default(),
            duration: 300,
        }
    }
}
