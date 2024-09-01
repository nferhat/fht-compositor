mod animation;
mod decoration;
mod input;
mod rules;

use std::collections::HashMap;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use smithay::reexports::rustix::path::Arg;

pub use self::animation::*;
pub use self::decoration::*;
pub use self::input::*;
pub use self::rules::*;
use crate::input::{KeyAction, KeyPattern, MouseAction, MousePattern};
use crate::shell::workspaces::WorkspaceLayout;

const fn default_true() -> bool {
    true
}

fn default_layouts() -> Vec<WorkspaceLayout> {
    vec![WorkspaceLayout::Tile]
}

const fn default_nmaster() -> usize {
    1
}

const fn default_mwfact() -> f32 {
    0.5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositorConfig {
    #[serde(default)]
    pub autostart: Vec<String>,

    #[serde(default)]
    pub greet: bool,

    #[serde(default)]
    pub keybinds: IndexMap<KeyPattern, KeyAction>,

    #[serde(default)]
    pub mousebinds: IndexMap<MousePattern, MouseAction>,

    #[serde(default)]
    pub input: InputConfig,

    #[serde(default)]
    pub general: GeneralConfig,

    #[serde(default)]
    pub decoration: DecorationConfig,

    #[serde(default)]
    pub animation: AnimationConfig,

    #[serde(default)]
    pub rules: HashMap<Vec<WindowPattern>, WindowRules>,

    #[serde(default)]
    pub renderer: RenderConfig,
}

impl Default for CompositorConfig {
    fn default() -> Self {
        Self {
            autostart: Vec::new(),
            greet: false,
            keybinds: IndexMap::new(),
            mousebinds: IndexMap::new(),
            input: InputConfig::default(),
            general: GeneralConfig::default(),
            decoration: DecorationConfig::default(),
            animation: AnimationConfig::default(),
            rules: HashMap::new(),
            renderer: RenderConfig::default(),
        }
    }
}

impl fht_config::Config for CompositorConfig {
    const NAME: &'static str = "compositor";
    const DEFAULT_CONTENTS: &'static str = include_str!("../../../res/compositor.ron");
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_true")]
    pub cursor_warps: bool,

    #[serde(default = "default_true")]
    pub focus_new_windows: bool,

    #[serde(default)]
    pub insert_window_strategy: InsertWindowStrategy,

    #[serde(default)]
    pub cursor: CursorConfig,

    #[serde(default = "default_layouts")]
    pub layouts: Vec<WorkspaceLayout>,

    #[serde(default = "default_nmaster")]
    pub nmaster: usize,

    #[serde(default = "default_mwfact")]
    pub mwfact: f32,

    #[serde(default)]
    pub outer_gaps: i32,

    #[serde(default)]
    pub inner_gaps: i32,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            cursor_warps: true,
            focus_new_windows: true,
            insert_window_strategy: InsertWindowStrategy::default(),
            cursor: CursorConfig::default(),
            layouts: vec![WorkspaceLayout::Tile],
            nmaster: 1,
            mwfact: 0.5,
            outer_gaps: 0,
            inner_gaps: 0,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, Hash)]
pub enum InsertWindowStrategy {
    #[default]
    EndOfSlaveStack,
    ReplaceMaster,
    AfterFocused,
}

fn default_cursor_theme() -> String {
    std::env::var("XCURSOR_THEME")
        .ok()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "default".to_string())
}

fn default_cursor_size() -> u32 {
    std::env::var("XCURSOR_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(24)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorConfig {
    #[serde(default = "default_cursor_theme")]
    pub name: String,

    #[serde(default = "default_cursor_size")]
    pub size: u32,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            name: default_cursor_theme(),
            size: default_cursor_size(),
        }
    }
}

#[cfg(feature = "udev_backend")]
fn default_disable_10bit() -> bool {
    std::env::var("FHTC_DISABLE_10_BIT")
        .ok()
        .and_then(|str| str.parse::<bool>().ok())
        .unwrap_or(false)
}

#[cfg(feature = "udev_backend")]
fn default_disable_overlay_planes() -> bool {
    std::env::var("FHTC_DISABLE_OVERLAY_PLANES")
        .ok()
        .and_then(|str| str.parse::<bool>().ok())
        .unwrap_or(false)
}

#[cfg(feature = "udev_backend")]
fn default_render_node() -> Option<std::path::PathBuf> {
    std::env::var("FHTC_RENDER_NODE")
        .ok()
        .map(std::path::PathBuf::from)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderConfig {
    #[cfg(feature = "udev_backend")]
    #[serde(default = "default_disable_10bit")]
    pub disable_10bit: bool,

    #[cfg(feature = "udev_backend")]
    #[serde(default = "default_disable_overlay_planes")]
    pub disable_overlay_planes: bool,

    #[cfg(feature = "udev_backend")]
    #[serde(default = "default_render_node")]
    pub render_node: Option<std::path::PathBuf>,

    #[serde(default)]
    pub damage_color: Option<[f32; 4]>,

    #[serde(default)]
    pub debug_overlay: bool,

    #[serde(default)]
    pub tile_debug_overlay: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            #[cfg(feature = "udev_backend")]
            disable_10bit: default_disable_10bit(),
            #[cfg(feature = "udev_backend")]
            disable_overlay_planes: default_disable_overlay_planes(),
            damage_color: None,
            #[cfg(feature = "udev_backend")]
            render_node: default_render_node(),
            debug_overlay: false,
            tile_debug_overlay: false,
        }
    }
}
