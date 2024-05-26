use serde::{Deserialize, Serialize};

use crate::utils::animation::curve::AnimationCurve;

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
    pub curve: AnimationCurve,
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
            curve: AnimationCurve::default(),
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
    pub curve: AnimationCurve,
    /// The duration of the animation, in milliseconds.
    #[serde(default = "default_window_animation_duration")]
    pub duration: u64,
}

impl Default for WindowOpenCloseAnimation {
    fn default() -> Self {
        Self {
            curve: AnimationCurve::default(),
            duration: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowGeometryAnimation {
    /// What easing to use for the animation:
    #[serde(default)]
    pub curve: AnimationCurve,
    /// The duration of the animation, in milliseconds.
    #[serde(default = "default_window_animation_duration")]
    pub duration: u64,
}

impl Default for WindowGeometryAnimation {
    fn default() -> Self {
        Self {
            curve: AnimationCurve::default(),
            duration: 300,
        }
    }
}
