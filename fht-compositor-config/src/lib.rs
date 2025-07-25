//! Library for configuration types definitions and configuration file loading using [`toml`] and
//! [`serde`]

#[macro_use]
extern crate tracing;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::num::NonZero;
use std::path::PathBuf;
use std::time::Duration;
use std::{fs, path};

use fht_animation::AnimationCurve;
use regex::Regex;
use serde::de::Unexpected;
use serde::{Deserialize, Deserializer};
use smithay::backend::input::MouseButton as SmithayMouseButton;
use smithay::input::keyboard::{
    keysyms, xkb, Keysym, ModifiersState as SmithayModifiersState, XkbConfig,
};
use smithay::reexports::input::{AccelProfile, ClickMethod, ScrollMethod, TapButtonMap};
use smithay::utils::Transform as SmithayTransform;
use toml::{Table, Value};

static DEFAULT_CONFIG_CONTENTS: &'static str = include_str!("../../res/compositor.toml");

const fn default_true() -> bool {
    true
}

const fn default_false() -> bool {
    false
}

fn default_keybinds() -> HashMap<KeyPattern, KeyActionDesc> {
    HashMap::from_iter([
        (
            KeyPattern(
                ModifiersState {
                    logo: true,
                    ..Default::default()
                },
                keysyms::KEY_Q.into(),
            ),
            KeyActionDesc::Simple(SimpleKeyAction::Quit),
        ),
        (
            KeyPattern(
                ModifiersState {
                    logo: true,
                    ..Default::default()
                },
                keysyms::KEY_R.into(),
            ),
            KeyActionDesc::Simple(SimpleKeyAction::ReloadConfig),
        ),
    ])
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
    pub autostart: Vec<String>,
    pub env: HashMap<String, String>,
    #[serde(default = "default_keybinds")]
    pub keybinds: HashMap<KeyPattern, KeyActionDesc>,
    pub mousebinds: HashMap<MousePattern, MouseAction>,
    pub input: Input,
    pub general: General,
    pub cursor: Cursor,
    pub decorations: Decorations,
    pub animations: Animations,
    pub rules: Vec<WindowRule>,
    pub layer_rules: Vec<LayerRule>,
    pub outputs: HashMap<String, Output>,
    pub debug: Debug,
}

// Custom default implementation to use default_keybinds() as the true default
// We don't want to have an empty keybind table when the user starts the compositor and then is
// unable to quit.
impl Default for Config {
    fn default() -> Self {
        Self {
            autostart: Default::default(),
            env: Default::default(),
            keybinds: default_keybinds(),
            mousebinds: Default::default(),
            input: Default::default(),
            general: Default::default(),
            cursor: Default::default(),
            decorations: Default::default(),
            animations: Default::default(),
            rules: Default::default(),
            layer_rules: Default::default(),
            outputs: HashMap::new(),
            debug: Default::default(),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Hash, PartialEq, Eq)]
pub struct ModifiersState {
    alt: bool,
    alt_gr: bool,
    ctrl: bool,
    logo: bool,
    shift: bool,
}

impl From<SmithayModifiersState> for ModifiersState {
    fn from(value: SmithayModifiersState) -> Self {
        Self {
            alt: value.alt,
            alt_gr: value.iso_level3_shift,
            ctrl: value.ctrl,
            logo: value.logo,
            shift: value.shift,
        }
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyPattern(pub ModifiersState, pub Keysym);

impl<'de> Deserialize<'de> for KeyPattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        // Very simple emacs-like key pattern. The example key patterns are:
        // Super-c, Logo-c, Mod-c, M-s
        // Shift-a, S-c
        // Alt-c, A-c
        // C-c, A-C-c, A-/
        let mut modifiers = ModifiersState::default();
        let mut keysym = None;
        for part in raw.split('-') {
            if keysym.is_some() {
                // We specified someting after having a keysym, invalid
                return Err(<D::Error as serde::de::Error>::custom(
                    "key pattern ends after the keysym",
                ));
            }

            match part.trim() {
                "Super" | "Mod" | "Logo" | "Meta" | "M" => modifiers.logo = true,
                "Shift" | "S" => modifiers.shift = true,
                "Alt" | "A" => modifiers.alt = true,
                "Ctrl" | "Control" | "C" => modifiers.ctrl = true,
                "AltGr" => modifiers.alt_gr = true,
                value => {
                    // We tried all the possible modifier patterns that we support
                    // Try to get a keysym from xkb, if we can't get the keysym, we can't build the
                    // keysym, and error out
                    match xkb::keysym_from_name(value, xkb::KEYSYM_NO_FLAGS).raw() {
                        keysyms::KEY_NoSymbol => {
                            match xkb::keysym_from_name(value, xkb::KEYSYM_CASE_INSENSITIVE).raw() {
                                keysyms::KEY_NoSymbol => {
                                    return Err(<D::Error as serde::de::Error>::invalid_value(
                                        Unexpected::Str(value),
                                        &"Keysym",
                                    ))
                                }
                                k => keysym = Some(k.into()),
                            }
                        }
                        k => keysym = Some(k.into()),
                    }
                }
            }
        }

        let Some(keysym) = keysym else {
            return Err(<D::Error as serde::de::Error>::missing_field("keysym"));
        };

        Ok(KeyPattern(modifiers, keysym))
    }
}

// Key action representation
// We use two enum variants in order to represent them, so that we can use the following syntax
// when specifying simple key actions
// ```toml
// Super-Shift-q = "quit"
// # This is still right, but the above is more ergonomic
// Super-Shift-q.action = "quit"
// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, untagged)]
pub enum KeyActionDesc {
    Simple(SimpleKeyAction),
    Complex {
        #[serde(flatten)]
        action: ComplexKeyAction,
        // HACK: rename_all = "kebab-case" does not affect enum struct fields.
        #[serde(default)]
        #[serde(rename = "allow-while-locked")]
        allow_while_locked: bool,
        #[serde(default)]
        #[serde(rename = "repeat")]
        repeat: bool,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum SimpleKeyAction {
    Quit,
    ReloadConfig,
    SelectNextLayout,
    SelectPreviousLayout,
    MaximizeFocusedWindow,
    FullscreenFocusedWindow,
    FloatFocusedWindow,
    CenterFloatingWindow,
    FocusNextWindow,
    FocusPreviousWindow,
    SwapWithNextWindow,
    SwapWithPreviousWindow,
    FocusNextOutput,
    FocusPreviousOutput,
    FocusNextWorkspace,
    FocusPreviousWorkspace,
    CloseFocusedWindow,
    None,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[serde(tag = "action", content = "arg")]
pub enum ComplexKeyAction {
    // Also include simple key actions here, since complex key action format will also be used
    // soon to include additional attributes for simple key actions (repeat,
    // allow-while-locked, etc...)
    Quit,
    ReloadConfig,
    SelectNextLayout,
    SelectPreviousLayout,
    MaximizeFocusedWindow,
    FullscreenFocusedWindow,
    FloatFocusedWindow,
    CenterFloatingWindow,
    MoveFloatingWindow([i32; 2]),
    ResizeFloatingWindow([i32; 2]),
    FocusNextWindow,
    FocusPreviousWindow,
    SwapWithNextWindow,
    SwapWithPreviousWindow,
    FocusNextOutput,
    FocusPreviousOutput,
    FocusNextWorkspace,
    FocusPreviousWorkspace,
    CloseFocusedWindow,
    None,
    RunCommand(String),
    ChangeMwfact(f64),
    ChangeNmaster(i32),
    ChangeWindowProportion(f64),
    FocusWorkspace(usize),
    SendToWorkspace(usize),
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Forward,
    Back,
}

impl From<SmithayMouseButton> for MouseButton {
    fn from(value: SmithayMouseButton) -> Self {
        match value {
            SmithayMouseButton::Left => Self::Left,
            SmithayMouseButton::Middle => Self::Middle,
            SmithayMouseButton::Right => Self::Right,
            SmithayMouseButton::Forward => Self::Forward,
            SmithayMouseButton::Back => Self::Back,
            _ => unreachable!(),
        }
    }
}

impl MouseButton {
    pub fn button_code(&self) -> u32 {
        // These are from linux/input-event-codes.h
        match self {
            MouseButton::Left => 0x110,
            MouseButton::Middle => 0x111,
            MouseButton::Right => 0x112,
            MouseButton::Forward => 0x115,
            MouseButton::Back => 0x116,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MousePattern(pub ModifiersState, pub MouseButton);

impl<'de> Deserialize<'de> for MousePattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let mut modifiers = ModifiersState::default();
        let mut button = None;
        for part in raw.split('-') {
            if button.is_some() {
                // We specified someting after having a keysym, invalid
                return Err(<D::Error as serde::de::Error>::custom(
                    "key pattern ends after the keysym",
                ));
            }

            match part.trim() {
                "Super" | "Mod" | "Logo" | "Meta" | "M" => modifiers.logo = true,
                "Shift" | "S" => modifiers.shift = true,
                "Alt" | "A" => modifiers.alt = true,
                "Ctrl" | "Control" | "C" => modifiers.ctrl = true,
                "AltGr" => modifiers.alt_gr = true,
                x => match x.to_lowercase().trim() {
                    "left" => button = Some(MouseButton::Left),
                    "middle" => button = Some(MouseButton::Middle),
                    "right" => button = Some(MouseButton::Right),
                    "forward" => button = Some(MouseButton::Forward),
                    "back" | "backwards" => button = Some(MouseButton::Back),
                    _ => {
                        return Err(<D::Error as serde::de::Error>::invalid_value(
                            Unexpected::Str(x),
                            &"MouseButton",
                        ))
                    }
                },
            }
        }

        let Some(button) = button else {
            return Err(<D::Error as serde::de::Error>::missing_field("button"));
        };

        Ok(MousePattern(modifiers, button))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum MouseAction {
    SwapTile,
    ResizeTile,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Input {
    pub keyboard: Keyboard,
    /// Configuration for generic mice.
    pub mouse: Mouse,
    /// Configuration for touchpads.
    pub touchpad: Mouse,
    /// Configuration for trackpoints, usually found on thinkpads.
    pub trackpoint: Mouse,
    pub per_device: HashMap<String, PerDeviceInput>,
}

fn default_keyboard_layout() -> String {
    "us".to_string()
}

const fn default_repeat_rate() -> NonZero<i32> {
    unsafe { NonZero::new_unchecked(25) }
}

const fn default_repeat_delay() -> NonZero<u64> {
    unsafe { NonZero::new_unchecked(250) }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Keyboard {
    pub rules: String,
    pub model: String,
    #[serde(default = "default_keyboard_layout")]
    pub layout: String,
    pub variant: String,
    pub options: String,
    #[serde(default = "default_repeat_delay")]
    pub repeat_delay: NonZero<u64>,
    #[serde(default = "default_repeat_rate")]
    pub repeat_rate: NonZero<i32>,
}

impl Default for Keyboard {
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

impl Keyboard {
    pub fn xkb_config(&self) -> XkbConfig {
        XkbConfig {
            rules: &self.rules,
            model: &self.model,
            layout: &self.layout,
            variant: &self.variant,
            options: Some(self.options.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ScrollMethodDef {
    NoScroll,
    TwoFinger,
    Edge,
    OnButtonDown,
}
impl Into<ScrollMethod> for ScrollMethodDef {
    fn into(self) -> ScrollMethod {
        match self {
            ScrollMethodDef::NoScroll => ScrollMethod::NoScroll,
            ScrollMethodDef::TwoFinger => ScrollMethod::TwoFinger,
            ScrollMethodDef::Edge => ScrollMethod::Edge,
            ScrollMethodDef::OnButtonDown => ScrollMethod::OnButtonDown,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum TapButtonMapDef {
    LeftRightMiddle,
    LeftMiddleRight,
}
impl Into<TapButtonMap> for TapButtonMapDef {
    fn into(self) -> TapButtonMap {
        match self {
            TapButtonMapDef::LeftRightMiddle => TapButtonMap::LeftRightMiddle,
            TapButtonMapDef::LeftMiddleRight => TapButtonMap::LeftMiddleRight,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum AccelProfileDef {
    Flat,
    Adaptive,
}
impl Into<AccelProfile> for AccelProfileDef {
    fn into(self) -> AccelProfile {
        match self {
            AccelProfileDef::Flat => AccelProfile::Flat,
            AccelProfileDef::Adaptive => AccelProfile::Adaptive,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ClickMethodDef {
    ButtonAreas,
    Clickfinger,
}
impl Into<ClickMethod> for ClickMethodDef {
    fn into(self) -> ClickMethod {
        match self {
            ClickMethodDef::ButtonAreas => ClickMethod::ButtonAreas,
            ClickMethodDef::Clickfinger => ClickMethod::Clickfinger,
        }
    }
}

#[derive(Default, Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Mouse {
    pub acceleration_profile: Option<AccelProfileDef>,
    pub acceleration_speed: Option<f64>,
    pub left_handed: Option<bool>,
    pub scroll_method: Option<ScrollMethodDef>,
    pub scroll_button_lock: Option<bool>,
    pub scroll_button: Option<MouseButton>,
    pub click_method: Option<ClickMethodDef>,
    pub natural_scrolling: Option<bool>,
    pub middle_button_emulation: Option<bool>,
    pub disable_while_typing: Option<bool>,
    pub tap_to_click: Option<bool>,
    pub tap_button_map: Option<TapButtonMapDef>,
    pub tap_and_drag: Option<bool>,
    pub drag_lock: Option<bool>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct PerDeviceInput {
    pub disable: bool,
    // NOTE: For now this is irrelevant since all keyboard config is global to wl_seat
    // pub keyboard: PerDeviceKeyboard,
    pub mouse: Mouse,
}

fn default_layouts() -> Vec<WorkspaceLayout> {
    vec![WorkspaceLayout::Tile]
}

const fn default_nmaster() -> NonZero<usize> {
    unsafe { NonZero::new_unchecked(1) }
}

const fn default_mwfact() -> f64 {
    0.5
}

const fn default_gaps() -> i32 {
    8
}

fn deserialize_layouts<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<WorkspaceLayout>, D::Error> {
    let value = Vec::<WorkspaceLayout>::deserialize(deserializer)?;
    if value.is_empty() {
        return Err(serde::de::Error::invalid_value(
            Unexpected::Seq,
            &"Non-empty list",
        ));
    }

    Ok(value)
}

fn deserialize_mwfact<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    let value = f64::deserialize(deserializer)?;
    if !(1e-3..=0.999).contains(&value) {
        return Err(serde::de::Error::invalid_value(
            Unexpected::Float(value),
            &"A value in 1e-3..=0.999",
        ));
    }

    Ok(value)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct General {
    #[serde(default = "default_true")]
    pub cursor_warps: bool,
    #[serde(default = "default_true")]
    pub focus_new_windows: bool,
    #[serde(default = "default_false")]
    pub focus_follows_mouse: bool,
    pub insert_window_strategy: InsertWindowStrategy,
    #[serde(default = "default_layouts", deserialize_with = "deserialize_layouts")]
    pub layouts: Vec<WorkspaceLayout>,
    #[serde(default = "default_nmaster")]
    pub nmaster: NonZero<usize>,
    #[serde(default = "default_mwfact", deserialize_with = "deserialize_mwfact")]
    pub mwfact: f64,
    #[serde(default = "default_gaps")]
    pub outer_gaps: i32,
    #[serde(default = "default_gaps")]
    pub inner_gaps: i32,
}

impl Default for General {
    fn default() -> Self {
        Self {
            cursor_warps: true,
            focus_new_windows: true,
            focus_follows_mouse: false,
            insert_window_strategy: InsertWindowStrategy::default(),
            layouts: default_layouts(),
            nmaster: default_nmaster(),
            mwfact: default_mwfact(),
            outer_gaps: default_gaps(),
            inner_gaps: default_gaps(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum WorkspaceLayout {
    Tile,
    BottomStack,
    CenteredMaster,
    Floating,
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum InsertWindowStrategy {
    #[default]
    EndOfSlaveStack,
    ReplaceMaster,
    AfterFocused,
}

fn default_cursor_theme() -> String {
    std::env::var("XCURSOR_THEME")
        .ok()
        .unwrap_or_else(|| "default".to_string())
}

fn default_cursor_size() -> u32 {
    std::env::var("XCURSOR_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(24)
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Cursor {
    #[serde(default = "default_cursor_theme")]
    pub name: String,
    #[serde(default = "default_cursor_size")]
    pub size: u32,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            name: default_cursor_theme(),
            size: default_cursor_size(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Decorations {
    pub border: Border,
    pub shadow: Shadow,
    pub blur: Blur,
    pub decoration_mode: DecorationMode,
}

impl Default for Decorations {
    fn default() -> Self {
        Self {
            border: Default::default(),
            shadow: Default::default(),
            blur: Default::default(),
            decoration_mode: DecorationMode::default(),
        }
    }
}

const fn default_thickness() -> i32 {
    2
}

const fn default_radius() -> f32 {
    10.0
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Border {
    pub focused_color: Color,
    pub normal_color: Color,
    #[serde(default = "default_thickness")]
    pub thickness: i32,
    #[serde(default = "default_radius")]
    pub radius: f32,
}

impl Default for Border {
    fn default() -> Self {
        Self {
            focused_color: Color::Gradient {
                start: csscolorparser::parse("#5781b9").unwrap().to_array(),
                end: csscolorparser::parse("#7fc8db").unwrap().to_array(),
                angle: 0.0,
            },
            normal_color: Color::Solid(csscolorparser::parse("#222230").unwrap().to_array()),
            thickness: default_thickness(),
            radius: default_radius(),
        }
    }
}

const fn default_shadow_sigma() -> f32 {
    10.
}

const fn default_shadow_color() -> [f32; 4] {
    [0.0, 0.0, 0.0, 0.75]
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Shadow {
    pub disable: bool,
    pub floating_only: bool,
    #[serde(
        deserialize_with = "deserialize_color",
        default = "default_shadow_color"
    )]
    pub color: [f32; 4],
    #[serde(default = "default_shadow_sigma")]
    pub sigma: f32,
}

impl Shadow {
    pub fn with_overrides(&self, overrides: &ShadowOverrides) -> Self {
        let mut ret = *self;
        ret.disable = overrides.disable.unwrap_or(ret.disable);
        ret.color = overrides.color.unwrap_or(ret.color);
        ret.sigma = overrides.sigma.unwrap_or(ret.sigma);

        ret
    }
}

impl Default for Shadow {
    fn default() -> Self {
        Self {
            disable: false,
            floating_only: true,
            color: default_shadow_color(),
            sigma: default_shadow_sigma(),
        }
    }
}

const fn default_blur_passes() -> usize {
    2
}

const fn default_blur_radius() -> f32 {
    5.0
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Blur {
    pub disable: bool,
    #[serde(default = "default_blur_passes")]
    pub passes: usize,
    #[serde(default = "default_blur_radius")]
    pub radius: f32,
    #[serde(default)]
    pub noise: f32,
}

impl Default for Blur {
    fn default() -> Self {
        Self {
            disable: false,
            passes: default_blur_passes(),
            radius: default_blur_radius(),
            noise: 0.0,
        }
    }
}

impl Blur {
    pub const DISABLED: Self = Self {
        disable: true,
        passes: 0,
        radius: 0.0,
        noise: 0.0,
    };

    pub fn disabled(&self) -> bool {
        self.disable || self.passes == 0 || self.radius == 0.0
    }

    pub fn with_overrides(&self, overrides: &BlurOverrides) -> Self {
        let mut ret = *self;
        if let Some(disable) = overrides.disable {
            ret.disable = disable;
        }
        if let Some(radius) = overrides.radius {
            ret.radius = radius;
        }
        if let Some(passes) = overrides.passes {
            ret.passes = passes;
        }
        if let Some(noise) = overrides.noise {
            ret.noise = noise;
        }

        ret
    }
}

#[derive(Default, Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum DecorationMode {
    ClientPreference,
    #[default]
    PreferServerSide,
    PreferClientSide,
    ForceServerSide,
    ForceClientSide,
}

impl Border {
    pub fn with_overrides(&self, overrides: &BorderOverrides) -> Self {
        let mut ret = *self;
        if let Some(focused_color) = &overrides.focused_color {
            ret.focused_color = *focused_color;
        }
        if let Some(normal_color) = &overrides.normal_color {
            ret.normal_color = *normal_color;
        }
        if let Some(thickness) = &overrides.thickness {
            ret.thickness = *thickness;
        }
        if let Some(radius) = &overrides.radius {
            ret.radius = *radius;
        }

        ret
    }
}

fn deserialize_color<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[f32; 4], D::Error> {
    csscolorparser::Color::deserialize(deserializer).map(|c| c.to_array())
}

fn deserialize_color_maybe<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<[f32; 4]>, D::Error> {
    let color = Option::<csscolorparser::Color>::deserialize(deserializer)?;
    Ok(color.map(|c| c.to_array()))
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[serde(untagged)]
pub enum Color {
    Solid(#[serde(deserialize_with = "deserialize_color")] [f32; 4]),
    Gradient {
        #[serde(deserialize_with = "deserialize_color")]
        start: [f32; 4],
        #[serde(deserialize_with = "deserialize_color")]
        end: [f32; 4],
        angle: f32,
    },
}

impl Color {
    pub fn components(&self) -> [f32; 4] {
        match self {
            Self::Solid(color) => *color,
            Self::Gradient { start, .. } => *start,
        }
    }
}

fn deserialize_duration_millis<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Duration, D::Error> {
    let value = u64::deserialize(deserializer)?;
    Ok(Duration::from_millis(value))
}

#[derive(Default, Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Animations {
    pub disable: bool,
    pub workspace_switch: WorkspaceSwitchAnimation,
    pub window_open_close: WindowOpenCloseAnimation,
    pub window_geometry: WindowGeometryAnimation,
}

const fn default_workspace_switch_animation_duration() -> Duration {
    Duration::from_millis(350)
}

fn default_workspace_switch_curve() -> AnimationCurve {
    fht_animation::SpringCurve::new(1.0, false, 0.85, 1.0, 600.0, Some(0.0001)).into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct WorkspaceSwitchAnimation {
    #[serde(default = "default_false")]
    pub disable: bool,
    pub direction: WorkspaceSwitchAnimationDirection,
    #[serde(default = "default_workspace_switch_curve")]
    pub curve: AnimationCurve,
    #[serde(
        default = "default_workspace_switch_animation_duration",
        serialize_with = "serialize_duration",
        deserialize_with = "deserialize_duration_millis"
    )]
    pub duration: Duration,
}

impl Default for WorkspaceSwitchAnimation {
    fn default() -> Self {
        Self {
            disable: false,
            curve: default_workspace_switch_curve(),
            duration: default_workspace_switch_animation_duration(),
            direction: WorkspaceSwitchAnimationDirection::Horizontal,
        }
    }
}

#[derive(Default, Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum WorkspaceSwitchAnimationDirection {
    #[default]
    Horizontal,
    Vertical,
}

const fn default_window_animation_duration() -> Duration {
    Duration::from_millis(300)
}

fn default_window_animation_curve() -> AnimationCurve {
    fht_animation::SpringCurve::new(1.0, false, 1.0, 1.2, 800.0, Some(0.0001)).into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct WindowOpenCloseAnimation {
    #[serde(default = "default_false")]
    pub disable: bool,
    #[serde(default = "default_window_animation_curve")]
    pub curve: AnimationCurve,
    #[serde(
        default = "default_window_animation_duration",
        serialize_with = "serialize_duration",
        deserialize_with = "deserialize_duration_millis"
    )]
    pub duration: Duration,
}

impl Default for WindowOpenCloseAnimation {
    fn default() -> Self {
        Self {
            disable: false,
            curve: default_window_animation_curve(),
            duration: default_workspace_switch_animation_duration(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct WindowGeometryAnimation {
    #[serde(default = "default_false")]
    pub disable: bool,
    #[serde(default = "default_window_animation_curve")]
    pub curve: AnimationCurve,
    #[serde(
        default = "default_window_animation_duration",
        serialize_with = "serialize_duration",
        deserialize_with = "deserialize_duration_millis"
    )]
    pub duration: Duration,
}

impl Default for WindowGeometryAnimation {
    fn default() -> Self {
        Self {
            disable: false,
            curve: default_window_animation_curve(),
            duration: default_window_animation_duration(),
        }
    }
}

fn deserialize_regexes<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Regex>, D::Error> {
    let patterns = Vec::<String>::deserialize(deserializer)?;
    let mut regexes = vec![];
    for pattern in patterns {
        let regex = Regex::new(&pattern).map_err(|err| {
            <D::Error as serde::de::Error>::custom(format!("Invalid regex string! {err}"))
        })?;
        regexes.push(regex);
    }

    Ok(regexes)
}

#[derive(Default, Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct WindowRule {
    // Matching requirements
    // When match_all == true, all the given window properties should have a match below
    pub match_all: bool,
    #[serde(deserialize_with = "deserialize_regexes")]
    pub match_title: Vec<Regex>,
    #[serde(deserialize_with = "deserialize_regexes")]
    pub match_app_id: Vec<Regex>,
    pub on_output: Option<String>,
    pub on_workspace: Option<usize>,
    pub is_focused: Option<bool>,
    pub is_floating: Option<bool>,
    // Rules to apply
    pub open_on_output: Option<String>,
    pub open_on_workspace: Option<usize>,
    pub border: BorderOverrides,
    pub blur: BlurOverrides,
    pub shadow: ShadowOverrides,
    pub proportion: Option<f64>,
    pub opacity: Option<f32>,
    pub decoration_mode: Option<DecorationMode>,
    pub maximized: Option<bool>,
    pub fullscreen: Option<bool>,
    pub floating: Option<bool>,
    pub ontop: Option<bool>,
    pub centered: Option<bool>, // only effective if floating == Some(true)
    pub vrr: Option<bool>,      // only effective when the window is on the primary plane
}

// NOTE: For layer shells we by default disable blur and shadow
fn default_blur_overrides_disabled() -> BlurOverrides {
    BlurOverrides {
        disable: Some(true),
        ..Default::default()
    }
}

fn default_shadow_overrides_disabled() -> ShadowOverrides {
    ShadowOverrides {
        disable: Some(true),
        ..Default::default()
    }
}

#[derive(Default, Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct LayerRule {
    // Matching requirements
    // When match_all == true, all the given layer properties should have a match below
    pub match_all: bool,
    #[serde(deserialize_with = "deserialize_regexes")]
    pub match_namespace: Vec<Regex>,
    pub on_output: Option<String>,
    // Rules to apply
    #[serde(default = "default_blur_overrides_disabled")]
    pub blur: BlurOverrides,
    #[serde(default = "default_shadow_overrides_disabled")]
    pub shadow: ShadowOverrides,
    pub opacity: Option<f32>,
    pub corner_radius: Option<f32>,
}

#[derive(Default, Debug, Clone, Copy, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct BorderOverrides {
    pub focused_color: Option<Color>,
    pub normal_color: Option<Color>,
    pub thickness: Option<i32>,
    pub radius: Option<f32>,
}

impl BorderOverrides {
    pub fn merge_with(mut self, other: Self) -> Self {
        if let Some(focused_color) = other.focused_color {
            self.focused_color = Some(focused_color);
        }
        if let Some(normal_color) = other.normal_color {
            self.normal_color = Some(normal_color);
        }
        if let Some(thickness) = other.thickness {
            self.thickness = Some(thickness);
        }
        if let Some(radius) = other.radius {
            self.radius = Some(radius);
        }

        self
    }
}

#[derive(Default, Debug, Clone, Copy, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct BlurOverrides {
    pub disable: Option<bool>,
    pub optimized: Option<bool>,
    pub passes: Option<usize>,
    pub radius: Option<f32>,
    pub noise: Option<f32>,
}

impl BlurOverrides {
    pub fn merge_with(mut self, other: Self) -> Self {
        if let Some(disable) = other.disable {
            self.disable = Some(disable);
        }
        if let Some(use_xray) = other.optimized {
            self.optimized = Some(use_xray);
        }
        if let Some(passes) = other.passes {
            self.passes = Some(passes);
        }
        if let Some(radius) = other.radius {
            self.radius = Some(radius);
        }
        if let Some(noise) = other.noise {
            self.noise = Some(noise);
        }

        self
    }
}

#[derive(Default, Debug, Clone, Copy, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct ShadowOverrides {
    pub disable: Option<bool>,
    #[serde(deserialize_with = "deserialize_color_maybe")]
    pub color: Option<[f32; 4]>,
    pub sigma: Option<f32>,
}

impl ShadowOverrides {
    pub fn merge_with(mut self, other: &Self) -> Self {
        if let Some(disable) = other.disable {
            self.disable = Some(disable);
        }
        if let Some(color) = other.color {
            self.color = Some(color);
        }
        if let Some(sigma) = other.sigma {
            self.sigma = Some(sigma);
        }

        self
    }
}

fn deserialize_output_mode<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<(u16, u16, Option<f64>)>, D::Error> {
    use sscanf::sscanf;
    let Some(raw) = Option::<String>::deserialize(deserializer)? else {
        return Ok(None);
    };

    let res = sscanf!(&raw, "{u16}x{u16}")
        .map(|(w, h)| (w, h, None))
        .map_err(|_| {
            <D::Error as serde::de::Error>::invalid_value(
                Unexpected::Str(&raw),
                &"{width}x{height}",
            )
        })
        .or_else(|_| {
            sscanf!(&raw, "{u16}x{u16}@{f64}")
                .map(|(w, h, refresh)| (w, h, Some(refresh)))
                .map_err(|_| {
                    <D::Error as serde::de::Error>::invalid_value(
                        Unexpected::Str(&raw),
                        &"{width}x{height}@{refresh}",
                    )
                })
        })?;
    Ok(Some(res))
}

#[derive(Default, Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum OutputTransform {
    #[default]
    #[serde(rename = "normal")]
    Normal,
    #[serde(rename = "90")]
    _90,
    #[serde(rename = "180")]
    _180,
    #[serde(rename = "270")]
    _270,
    #[serde(rename = "flipped")]
    Flipped,
    #[serde(rename = "flipped-90")]
    Flipped90,
    #[serde(rename = "flipped-180")]
    Flipped180,
    #[serde(rename = "flipped-270")]
    Flipped270,
}

impl Into<SmithayTransform> for OutputTransform {
    fn into(self) -> SmithayTransform {
        match self {
            Self::Normal => SmithayTransform::Normal,
            Self::_90 => SmithayTransform::_90,
            Self::_180 => SmithayTransform::_180,
            Self::_270 => SmithayTransform::_270,
            Self::Flipped => SmithayTransform::Flipped,
            Self::Flipped90 => SmithayTransform::Flipped90,
            Self::Flipped180 => SmithayTransform::Flipped180,
            Self::Flipped270 => SmithayTransform::Flipped270,
        }
    }
}

#[derive(Default, Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum VrrMode {
    On,
    #[default]
    Off,
    OnDemand,
}

#[derive(Default, Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct OutputPosition {
    pub x: i32,
    pub y: i32,
}

#[derive(Default, Debug, Clone, Deserialize, PartialEq)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Output {
    pub disable: bool,
    // Configured output mode, takes the form of (width, height, refresh (in hz))
    // If refresh rate is not specified, use the highest available.
    #[serde(deserialize_with = "deserialize_output_mode")]
    pub mode: Option<(u16, u16, Option<f64>)>,
    pub transform: Option<OutputTransform>,
    pub scale: Option<i32>,
    pub position: Option<OutputPosition>,
    pub vrr: VrrMode,
}

fn default_disable_10bit() -> bool {
    std::env::var("FHTC_DISABLE_10_BIT")
        .ok()
        .and_then(|str| str.parse::<bool>().ok())
        .unwrap_or(false)
}

fn default_disable_overlay_planes() -> bool {
    std::env::var("FHTC_DISABLE_OVERLAY_PLANES")
        .ok()
        .and_then(|str| str.parse::<bool>().ok())
        .unwrap_or(false)
}

fn default_render_node() -> Option<std::path::PathBuf> {
    std::env::var("FHTC_RENDER_NODE")
        .ok()
        .map(std::path::PathBuf::from)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Debug {
    #[serde(default = "default_disable_10bit")]
    pub disable_10bit: bool,
    #[serde(default = "default_disable_overlay_planes")]
    pub disable_overlay_planes: bool,
    #[serde(default = "default_render_node")]
    pub render_node: Option<std::path::PathBuf>,
    pub draw_damage: bool,
    pub draw_opaque_regions: bool,
    pub debug_overlay: bool,
    pub tile_debug_overlay: bool,
}

impl Default for Debug {
    fn default() -> Self {
        Self {
            disable_10bit: default_disable_10bit(),
            disable_overlay_planes: default_disable_overlay_planes(),
            render_node: default_render_node(),
            draw_damage: false,
            draw_opaque_regions: false,
            debug_overlay: false,
            tile_debug_overlay: false,
        }
    }
}

fn get_xdg_path() -> Result<path::PathBuf, xdg::BaseDirectoriesError> {
    xdg::BaseDirectories::new()
        .map(|base_directories| base_directories.get_config_file("fht/compositor.toml"))
}

fn fallback_path() -> path::PathBuf {
    // NOTE: Deprecation is only relevant on windows, where this library should never be used.
    #[allow(deprecated)]
    let mut path = std::env::home_dir().expect("No $HOME directory?");
    path.push("config");
    path.push("fht");
    path.push("compositor.toml");
    path
}

pub fn config_path() -> PathBuf {
    get_xdg_path().inspect_err(|err| {
            warn!(?err, "Failed to get config path from XDG! using fallback location: $HOME/.config/fht/compositor.toml");
    }).ok().unwrap_or_else(fallback_path)
}

pub fn load(path: Option<path::PathBuf>) -> Result<(Config, Vec<path::PathBuf>), Error> {
    let path = path.unwrap_or_else(config_path);
    debug!(?path, "Loading compositor configuration");

    let mut file = match fs::OpenOptions::new().read(true).write(false).open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            info!(?path, "Creating configuration file");
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }

            let mut new_file = fs::File::create(&path)?;
            new_file.write_all(DEFAULT_CONFIG_CONTENTS.as_bytes())?;

            fs::OpenOptions::new().read(true).open(&path)?
        }
        Err(err) => return Err(err.into()),
    };

    let mut buf = String::new();
    let _ = file.read_to_string(&mut buf)?;

    // First deserialize as a toml::Value and try to get the imports table to merge with.
    let mut config: Value = toml::de::from_str(buf.as_str())?;
    let mut paths = vec![path];

    if let Some(imports) = config.get("imports").cloned() {
        let imports = Value::try_into::<Vec<PathBuf>>(imports);
        if let Ok(imports) = imports {
            for mut path in imports {
                if let Ok(stripped) = path.strip_prefix("~/") {
                    // NOTE: Deprecation is only relevant on windows, where this library should
                    // never be used.
                    #[allow(deprecated)]
                    let home_dir = std::env::home_dir().expect("No $HOME directory?");
                    path = home_dir.join(stripped);
                }

                let mut file = match fs::OpenOptions::new().read(true).write(false).open(&path) {
                    Ok(file) => file,
                    Err(err) => {
                        error!(?err, ?path, "Failed to open import file");
                        continue;
                    }
                };
                let mut buf = String::new();
                if let Err(err) = file.read_to_string(&mut buf) {
                    error!(?err, "Failed to read import file");
                    continue;
                }
                match toml::de::from_str(&buf) {
                    Ok(value) => {
                        debug!(?path, "Merging configuration from path");
                        paths.push(path);
                        config = merge(config, value);
                    }
                    Err(err) => {
                        error!(?err, ?path, "Failed to read configuration from import path")
                    }
                }
            }
        }
    }

    if let Value::Table(table) = &mut config {
        // We dont want it inside the final config struct
        // If the config is not a table it will error down below so there's not need to error here
        table.remove("imports");
    }

    let config = Value::try_into(config)?;
    Ok((config, paths))
}

/// Merge two serde structures.
///
/// This will take all values from `replacement` and use `base` whenever a value isn't present in
/// `replacement`.
///
/// Copyright https://github.com/alacritty/alacritty under the apache 2.0 license
/// Thank you very much!
fn merge(base: Value, replacement: Value) -> Value {
    fn merge_tables(mut base: Table, replacement: Table) -> Table {
        for (key, value) in replacement {
            let value = match base.remove(&key) {
                Some(base_value) => merge(base_value, value),
                None => value,
            };
            base.insert(key, value);
        }

        base
    }

    match (base, replacement) {
        (Value::Array(mut base), Value::Array(mut replacement)) => {
            base.append(&mut replacement);
            Value::Array(base)
        }
        (Value::Table(base), Value::Table(replacement)) => {
            Value::Table(merge_tables(base, replacement))
        }
        (_, value) => value,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error occured when loading the configuration file: {0}")]
    IO(#[from] io::Error),
    #[error("An error occured while parsing the configuration file: {0}")]
    Parse(#[from] toml::de::Error),
}
