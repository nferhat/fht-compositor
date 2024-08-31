use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub use self::keyboard::KeyboardConfig;
pub use self::mouse::MouseConfig;

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct InputConfig {
    #[serde(default)]
    pub keyboard: KeyboardConfig,

    #[serde(default)]
    pub mouse: MouseConfig,

    #[serde(default)]
    pub per_device: IndexMap<String, PerDeviceInputConfig>,
}

// To avoid infinite recursion
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct PerDeviceInputConfig {
    #[serde(default)]
    pub disable: bool,

    #[serde(default)]
    pub keyboard: KeyboardConfig,

    #[serde(default)]
    pub mouse: MouseConfig,
}

mod keyboard {
    use serde::{Deserialize, Serialize};
    use smithay::input::keyboard::XkbConfig;

    fn default_keyboard_layout() -> String {
        "us".to_string()
    }

    const fn default_repeat_rate() -> i32 {
        25
    }

    const fn default_repeat_delay() -> i32 {
        250
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    pub struct KeyboardConfig {
        #[serde(default)]
        pub rules: String,

        #[serde(default)]
        pub model: String,

        #[serde(default = "default_keyboard_layout")]
        pub layout: String,

        #[serde(default)]
        pub variant: String,

        #[serde(default)]
        pub options: String,

        #[serde(default = "default_repeat_delay")]
        pub repeat_delay: i32,

        #[serde(default = "default_repeat_rate")]
        pub repeat_rate: i32,
    }

    impl Default for KeyboardConfig {
        fn default() -> Self {
            let default = XkbConfig::default();
            Self {
                rules: default.rules.to_string(),
                model: default.model.to_string(),
                layout: default.layout.to_string(),
                variant: default.variant.to_string(),
                options: default.options.unwrap_or_default(),

                repeat_delay: default_repeat_delay(),
                repeat_rate: default_repeat_rate(),
            }
        }
    }

    impl KeyboardConfig {
        pub fn get_xkb_config(&self) -> XkbConfig {
            XkbConfig {
                rules: &self.rules,
                model: &self.model,
                layout: &self.layout,
                variant: &self.variant,
                options: Some(self.options.clone()),
            }
        }
    }
}

mod mouse {
    use serde::{Deserialize, Serialize};
    use smithay::reexports::input::{AccelProfile, ScrollMethod, TapButtonMap};

    fn default_scrollmethod() -> ScrollMethod {
        ScrollMethod::TwoFinger
    }

    fn default_tap_to_click_behaviour() -> TapButtonMap {
        // An educated guess, I can't seem to find anything on the docs
        TapButtonMap::LeftRightMiddle
    }

    const fn default_accelprofile() -> AccelProfile {
        // Based on libinput docs this is the default
        AccelProfile::Adaptive
    }

    const fn default_accelspeed() -> f64 {
        1.0
    }

    const fn default_true() -> bool {
        true
    }

    const fn default_false() -> bool {
        false
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MouseConfig {
        #[serde(default = "default_accelprofile")]
        #[serde(serialize_with = "ser::serialize_accelprofile")]
        #[serde(deserialize_with = "ser::deserialize_accelprofile")]
        pub acceleration_profile: AccelProfile,

        #[serde(default = "default_accelspeed")]
        pub acceleration_speed: f64,

        #[serde(default = "default_false")]
        pub left_handed: bool,

        #[serde(default = "default_scrollmethod")]
        #[serde(serialize_with = "ser::serialize_scrollmethod")]
        #[serde(deserialize_with = "ser::deserialize_scrollmethod")]
        pub scroll_method: ScrollMethod,

        #[serde(default = "default_false")]
        pub natural_scrolling: bool,

        #[serde(default = "default_false")]
        pub middle_button_emulation: bool,

        #[serde(default = "default_true")]
        pub disable_while_typing: bool,

        #[serde(default = "default_false")]
        pub tap_to_click: bool,

        #[serde(default = "default_tap_to_click_behaviour")]
        #[serde(serialize_with = "ser::serialize_tap_to_click_behaviour")]
        #[serde(deserialize_with = "ser::deserialize_tap_to_click_behaviour")]
        pub tap_to_click_behaviour: TapButtonMap,

        #[serde(default = "default_true")]
        pub tap_and_drag: bool,
    }

    impl Default for MouseConfig {
        fn default() -> Self {
            Self {
                acceleration_profile: default_accelprofile(),
                acceleration_speed: default_accelspeed(),
                left_handed: default_false(),
                scroll_method: default_scrollmethod(),
                natural_scrolling: default_false(),
                middle_button_emulation: default_false(),
                disable_while_typing: default_true(),
                tap_to_click: default_false(),
                tap_to_click_behaviour: default_tap_to_click_behaviour(),
                tap_and_drag: default_true(),
            }
        }
    }

    mod ser {
        use serde::{Deserialize, Deserializer, Serializer};
        use smithay::reexports::input::{AccelProfile, ScrollMethod, TapButtonMap};

        pub fn serialize_accelprofile<S: Serializer>(
            accel_profile: &AccelProfile,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            serializer.serialize_u8(*accel_profile as u8)
        }

        pub fn deserialize_accelprofile<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<AccelProfile, D::Error> {
            let value = u8::deserialize(deserializer)?;
            match value {
                0 => Ok(AccelProfile::Flat),
                1 => Ok(AccelProfile::Adaptive),
                _ => Err(<D::Error as serde::de::Error>::invalid_value(
                    serde::de::Unexpected::Unsigned(value as u64),
                    &"Acceleration profile doesnt exist!",
                )),
            }
        }

        pub fn serialize_scrollmethod<S: Serializer>(
            scroll_method: &ScrollMethod,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            serializer.serialize_u8(*scroll_method as u8)
        }

        pub fn deserialize_scrollmethod<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<ScrollMethod, D::Error> {
            let value = u8::deserialize(deserializer)?;
            match value {
                0 => Ok(ScrollMethod::NoScroll),
                1 => Ok(ScrollMethod::TwoFinger),
                2 => Ok(ScrollMethod::Edge),
                3 => Ok(ScrollMethod::OnButtonDown),
                _ => Err(<D::Error as serde::de::Error>::invalid_value(
                    serde::de::Unexpected::Unsigned(value as u64),
                    &"Input profile doesnt exist!",
                )),
            }
        }

        pub fn serialize_tap_to_click_behaviour<S: Serializer>(
            tap_to_click_behaviour: &TapButtonMap,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            serializer.serialize_u8(*tap_to_click_behaviour as u8)
        }

        pub fn deserialize_tap_to_click_behaviour<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<TapButtonMap, D::Error> {
            let value = u8::deserialize(deserializer)?;
            match value {
                0 => Ok(TapButtonMap::LeftRightMiddle),
                1 => Ok(TapButtonMap::LeftMiddleRight),
                _ => Err(<D::Error as serde::de::Error>::invalid_value(
                    serde::de::Unexpected::Unsigned(value as u64),
                    &"Tap to click behaviour doesn't exist!",
                )),
            }
        }
    }
}
