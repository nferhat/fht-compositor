use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub use self::keyboard::KeyboardConfig;
pub use self::mouse::MouseConfig;

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct InputConfig {
    /// Keyboard specific settings.
    #[serde(default)]
    pub keyboard: KeyboardConfig,

    /// Mouse specific settings.
    #[serde(default)]
    pub mouse: MouseConfig,

    /// Per device settings.
    ///
    /// Each device config is the same as [`InputConfig`], just specific to a device.
    ///
    /// As far as I know [`KeyboardConfig`] is specific to the global wl_seat object, so this won't
    /// really affect anything, so eh, [`MouseConfig`] works though.
    ///
    /// NOTE: Having this set for a device will IGNORE any other global config.
    #[serde(default)]
    pub per_device: IndexMap<String, PerDeviceInputConfig>,
}

// To avoid infinite recursion
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct PerDeviceInputConfig {
    /// Whether to enable or disable this device, discarding any event coming from it.
    #[serde(default)]
    pub disable: bool,

    /// Keyboard specific settings for this device, if applicable.
    ///
    /// NOTE: this does nothing.
    #[serde(default)]
    pub keyboard: KeyboardConfig,

    /// Mouse specific settings for this device, if applicable.
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
        /// The rules file to use.
        ///
        /// The rules file describes how to interpret the values of the model, layout, variant and
        /// options fields.
        #[serde(default)]
        pub rules: String,

        /// The keyboard model by which to interpret keycodes and LEDs.
        #[serde(default)]
        pub model: String,

        /// A comma separated list of layouts (languages) to include in the keymap.
        #[serde(default = "default_keyboard_layout")]
        pub layout: String,

        /// A comma separated list of variants, one per layout, which may modify or augment the
        /// respective layout in various ways.
        #[serde(default)]
        pub variant: String,

        /// A comma separated list of options, through which the user specifies non-layout related
        /// preferences, like which key combinations are used for switching layouts, or which key
        /// is the Compose key.
        #[serde(default)]
        pub options: String,

        /// How much should the keyboard wait before starting repeating keys?
        #[serde(default = "default_repeat_delay")]
        pub repeat_delay: i32,

        /// How fast should the keyboard repeat inputs?
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
        /// The libinput [acceleration profile](https://wayland.freedesktop.org/libinput/doc/latest/pointer-acceleration.html#pointer-acceleration.)
        #[serde(default = "default_accelprofile")]
        #[serde(serialize_with = "ser::serialize_accelprofile")]
        #[serde(deserialize_with = "ser::deserialize_accelprofile")]
        pub acceleration_profile: AccelProfile,

        /// The libinput acceleration speed.
        #[serde(default = "default_accelspeed")]
        pub acceleration_speed: f64,

        /// Switches the left and right mouse buttons, to adapt for lefties.
        #[serde(default = "default_false")]
        pub left_handed: bool,

        /// How should we scroll using the touchpad only?
        ///
        /// NOTE: This setting is touchpad-specific
        #[serde(default = "default_scrollmethod")]
        #[serde(serialize_with = "ser::serialize_scrollmethod")]
        #[serde(deserialize_with = "ser::deserialize_scrollmethod")]
        pub scroll_method: ScrollMethod,

        /// Should we use [natural scrolling](https://wayland.freedesktop.org/libinput/doc/latest/scrolling.html#natural-scrolling-vs-traditional-scrolling)?
        ///
        /// NOTE: This setting is touchpad-specific
        #[serde(default = "default_false")]
        pub natural_scrolling: bool,

        /// Should we emulate a middle button click/scrollwheel button click when pressing both the
        /// left touchpad button and right touchpad button at the same time
        #[serde(default = "default_false")]
        pub middle_button_emulation: bool,

        /// Should we disable the touchpad while typing?
        ///
        /// NOTE: This setting is touchpad-specific
        #[serde(default = "default_true")]
        pub disable_while_typing: bool,

        /// Whether to enable [tap-to-click](https://wayland.freedesktop.org/libinput/doc/latest/tapping.html)
        #[serde(default = "default_false")]
        pub tap_to_click: bool,

        /// How should tap to click works, useful if tap_to_click is enabled.
        ///
        /// NOTE: This setting is touchpad-specific
        #[serde(default = "default_tap_to_click_behaviour")]
        #[serde(serialize_with = "ser::serialize_tap_to_click_behaviour")]
        #[serde(deserialize_with = "ser::deserialize_tap_to_click_behaviour")]
        pub tap_to_click_behaviour: TapButtonMap,

        /// Whether to enable [tap-and-drag](https://wayland.freedesktop.org/libinput/doc/latest/tapping.html#tap-and-drag)
        ///
        /// This is independent of the tap_to_click option.
        ///
        /// NOTE: This setting is touchpad-specific
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
