use serde::{Deserialize, Serialize};

use crate::utils::animation::curve::AnimationCurve;

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct AnimationConfig {
    #[serde(default)]
    pub workspace_switch: WorkspaceSwitchAnimationConfig,

    #[serde(default)]
    pub window_open_close: WindowOpenCloseAnimation,

    #[serde(default)]
    pub window_geometry: WindowGeometryAnimation,
}

const fn default_workspace_switch_animation_duration() -> u64 {
    350
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSwitchAnimationConfig {
    #[serde(default)]
    pub curve: AnimationCurve,
    #[serde(default = "default_workspace_switch_animation_duration")]
    pub duration: u64,
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
    #[serde(default)]
    pub curve: AnimationCurve,
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
    #[serde(default)]
    pub curve: AnimationCurve,
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
