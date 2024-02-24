use serde::{Deserialize, Serialize};
use tween::TweenValue;

#[derive(Default, Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Easing {
    BackIn,
    BackInOut,
    BackOut,
    BounceIn,
    BounceInOut,
    CircIn,
    CircInOut,
    CircOut,
    CubicIn,
    CubicInOut,
    CubicOut,
    ElasticIn,
    ElasticInOut,
    ElasticOut,
    ExpoIn,
    ExpoInOut,
    ExpoOut,
    #[default]
    Linear,
    QuadIn,
    QuadInOut,
    QuadOut,
    QuartIn,
    QuartInOut,
    QuartOut,
    QuintIn,
    QuintInOut,
    QuintOut,
    SineIn,
    SineInOut,
    SineOut,
}

impl Easing {
    pub fn tween<Value: TweenValue>(&self, value_delta: Value, percent: f32) -> Value {
        match self {
            Self::BackIn => tween::BackIn.tween(value_delta, percent),
            Self::BackInOut => tween::BackInOut.tween(value_delta, percent),
            Self::BackOut => tween::BackOut.tween(value_delta, percent),
            Self::BounceIn => tween::BounceIn.tween(value_delta, percent),
            Self::BounceInOut => tween::BounceInOut.tween(value_delta, percent),
            Self::CircIn => tween::CircIn.tween(value_delta, percent),
            Self::CircInOut => tween::CircInOut.tween(value_delta, percent),
            Self::CircOut => tween::CircOut.tween(value_delta, percent),
            Self::CubicIn => tween::CubicIn.tween(value_delta, percent),
            Self::CubicInOut => tween::CubicInOut.tween(value_delta, percent),
            Self::CubicOut => tween::CubicOut.tween(value_delta, percent),
            Self::ElasticIn => tween::ElasticIn.tween(value_delta, percent),
            Self::ElasticInOut => tween::ElasticInOut.tween(value_delta, percent),
            Self::ElasticOut => tween::ElasticOut.tween(value_delta, percent),
            Self::ExpoIn => tween::ExpoIn.tween(value_delta, percent),
            Self::ExpoInOut => tween::ExpoInOut.tween(value_delta, percent),
            Self::ExpoOut => tween::ExpoOut.tween(value_delta, percent),
            Self::Linear => tween::Linear.tween(value_delta, percent),
            Self::QuadIn => tween::QuadIn.tween(value_delta, percent),
            Self::QuadInOut => tween::QuadInOut.tween(value_delta, percent),
            Self::QuadOut => tween::QuadOut.tween(value_delta, percent),
            Self::QuartIn => tween::QuartIn.tween(value_delta, percent),
            Self::QuartInOut => tween::QuartInOut.tween(value_delta, percent),
            Self::QuartOut => tween::QuartOut.tween(value_delta, percent),
            Self::QuintIn => tween::QuintIn.tween(value_delta, percent),
            Self::QuintInOut => tween::QuintInOut.tween(value_delta, percent),
            Self::QuintOut => tween::QuintOut.tween(value_delta, percent),
            Self::SineIn => tween::SineIn.tween(value_delta, percent),
            Self::SineInOut => tween::SineInOut.tween(value_delta, percent),
            Self::SineOut => tween::SineOut.tween(value_delta, percent),
        }
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct AnimationConfig {
    /// The animation for workspaces switches
    #[serde(default)]
    pub workspace_switch: WorkspaceSwitchAnimationConfig,
}

const fn default_workspace_switch_animation_duration() -> f64 {
    350.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSwitchAnimationConfig {
    /// What easing to use for the animation:
    #[serde(default)]
    pub easing: Easing,
    /// The duration of the animation, in milliseconds.
    #[serde(default = "default_workspace_switch_animation_duration")]
    pub duration: f64,
    /// The direction, whether to switch vertically of horizontally.
    #[serde(default)]
    pub direction: WorkspaceSwitchAnimationDirection,
}

impl Default for WorkspaceSwitchAnimationConfig {
    fn default() -> Self {
        Self {
            easing: Easing::default(),
            duration: 350.0,
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
