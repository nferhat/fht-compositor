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
use crate::backend::render::BackendAllocator;
use crate::input::{KeyAction, KeyPattern, MouseAction, MousePattern};

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FhtConfig {
    /// A list of programs to autostart
    ///
    /// NOTE: These are evaluated using `/bin/sh`
    #[serde(default)]
    pub autostart: Vec<String>,

    /// Keybinds, table of key patterns bound to key actions.
    #[serde(default)]
    pub keybinds: IndexMap<KeyPattern, KeyAction>,

    /// Mousebinds, a table of mouse pattern bound to mouse actions.
    #[serde(default)]
    pub mousebinds: IndexMap<MousePattern, MouseAction>,

    /// Input configuration.
    #[serde(default)]
    pub input: InputConfig,

    /// General behaviour configuration.
    #[serde(default)]
    pub general: GeneralConfig,

    /// Decorations configuration.
    #[serde(default)]
    pub decoration: DecorationConfig,

    /// Different animations that fht-compositor provides you with.
    #[serde(default)]
    pub animation: AnimationConfig,

    /// Window rules.
    #[serde(default)]
    pub rules: HashMap<Vec<WindowRulePattern>, WindowMapSettings>,

    /// Configuration for the backend renderer.
    #[serde(default)]
    pub renderer: RenderConfig,
}

impl Default for FhtConfig {
    fn default() -> Self {
        Self {
            autostart: Vec::new(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    /// Should we warp the mouse cursor when focusing windows?
    ///
    /// If you use keybinds with the [`FocusNextWindow`] and [`FocusPreviousWindow`] actions,
    /// enabling this option will warp the mouse to the center of that window.
    ///
    /// NOTE: This doesn't work on the x11 backend.
    #[serde(default = "default_true")]
    pub cursor_warps: bool,

    /// Should new windows be focused automatically
    #[serde(default = "default_true")]
    pub focus_new_windows: bool,

    /// Cursor configuration.
    ///
    /// Basically the icon used to indicate *where* the pointer is.
    #[serde(default)]
    pub cursor: CursorConfig,

    /// Useless gap added around the output edge when tiling windows.
    #[serde(default)]
    pub outer_gaps: i32,

    /// Useless gap added between the windows when tiling them.
    #[serde(default)]
    pub inner_gaps: i32,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            warp_window_on_focus: true,
            focus_new_windows: true,
            cursor: CursorConfig::default(),
            outer_gaps: 0,
            inner_gaps: 0,
        }
    }
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
    /// The cursor theme name.
    ///
    /// This fallbacks to the `XCURSOR_THEME` environment variable if not set.
    ///
    /// NOTE: If you change this and reload the configuration, you have to restart every
    /// application in order for them to acknowledge the change.
    #[serde(default = "default_cursor_theme")]
    pub name: String,

    /// The cursor size.
    ///
    /// This fallbacks to the `XCURSOR_SIZE` environment variable if not set.
    ///
    /// NOTE: If you change this and reload the configuration, you have to restart every
    /// application in order for them to acknowledge the change.
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
    /// Which allocator to prefer when running a backend?
    ///
    /// If this is none, the Vulkan backend will be used
    #[serde(default)]
    pub allocator: BackendAllocator,

    /// Should we avoid using 10-bit color formats.
    ///
    /// This is only effective in the udev backend.
    #[cfg(feature = "udev_backend")]
    #[serde(default = "default_disable_10bit")]
    pub disable_10bit: bool,

    /// Should we disable overlay planes for the DRM compositor
    #[cfg(feature = "udev_backend")]
    #[serde(default = "default_disable_overlay_planes")]
    pub disable_overlay_planes: bool,

    /// What DRM node should the compositor use for rendering.
    #[cfg(feature = "udev_backend")]
    #[serde(default = "default_render_node")]
    pub render_node: Option<std::path::PathBuf>,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            allocator: BackendAllocator::default(),
            #[cfg(feature = "udev_backend")]
            disable_10bit: default_disable_10bit(),
            #[cfg(feature = "udev_backend")]
            disable_overlay_planes: default_disable_overlay_planes(),
            #[cfg(feature = "udev_backend")]
            render_node: default_render_node(),
        }
    }
}
