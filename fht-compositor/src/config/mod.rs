//! Compositor configuration.
//!
//! The config is located at `~/.config/fht/compositor.ron`, using the [ron](https://docs.rs/ron/0.8.1/ron/)
//! as the configuration format (go read their spec before anything else)
//!
//! ## Configuration reloading
//!
//! Live reloading is *not* supported (yet). Use the dedicated
//! [`ReloadConfig`](crate::input::actions::KeyAction::ReloadConfig) action to reload your
//! configuration.
//!
//! If your configuration fails to reload, fht-compositor will fallback on the following generic
//! configuration:
//!
//! ```rust
//! (
//!     autostart: [],
//!
//!     keybinds: {
//!         ([ALT], "q"): Quit,
//!         ([ALT], "r"): ReloadConfig,
//!     },
//! )
//! ```
//! </div>
//!
//!
//! A example config may look like the following:
//!
//! ```rust,ignore
//! (
//!     autostart: [
//!         "/usr/libexec/polkit-gnome-authentication-agent-1",
//!         "swaybg -i ~/.config/theme/wallpaper.jpg",
//!     ],
//!
//!     general: (
//!         warp_window_on_focus: false,
//!         outer_gaps: 8,
//!         inner_gaps: 8,
//!     ),
//!
//!     keybinds: {
//!         // Left side of the tuple: your modifiers
//!         // Right side of the tuple: the desired key
//!         //
//!         // You should go check the KeyPattern documentation for more info.
//!         //
//!         // You should go check the KeyAction enum for all possible actions
//!
//!         ([SUPER], "q"): Quit,
//!         ([SUPER, CTRL], "r"): ReloadConfig,
//!         ([SUPER], "Return"): RunCommand("alacritty"),
//!         ([SUPER], "p"): RunCommand("wofi --show drun"),
//!     },
//!
//!     input: (
//!         keyboard: (
//!             rules: "",
//!             model: "",
//!             layout: "us",
//!             variant: "",
//!             options: "",
//!
//!             repeat_rate: 50,
//!             repeat_delay: 250,
//!         ),
//!
//!         mouse: (
//!             acceleration_profile: Flat,
//!         ),
//!     ),
//!
//!     renderer: (
//!         allocator: Vulkan,
//!         // allocator: Gbm,
//!     )
//! )
//! ```
//!
//! ## TODO
//!
//! - [x] Cursor configuration
//! - [ ] Output configuration
//! - [ ] Window rules

mod types;

use std::cell::SyncUnsafeCell;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::LazyLock;

use anyhow::Context;
use smithay::reexports::input::{Device, DeviceCapability, SendEventsMode};
use xdg::BaseDirectories;

const DEFAULT_CONFIG: &str = include_str!("../../res/compositor.ron");

#[allow(unused_imports)]
pub use self::types::{
    AnimationConfig, BorderConfig, CursorConfig, Easing, FhtConfig as FhtConfigInner,
    GeneralConfig, InputConfig, KeyboardConfig, MouseConfig, PerDeviceInputConfig,
    WindowMapSettings, WindowRulePattern, WorkspaceSwitchAnimationConfig,
    WorkspaceSwitchAnimationDirection,
};

// To avoid mutable static madness just use an private unsafe cell with one getter and setter.
// Dont show this to the user though
#[derive(Debug, Default)]
#[doc(hidden)]
pub struct FhtConfig(SyncUnsafeCell<FhtConfigInner>);

impl std::ops::Deref for FhtConfig {
    type Target = FhtConfigInner;

    fn deref(&self) -> &Self::Target {
        // This is kinda scary todo, but we are using a LazyCell around this struct, so it should
        // always be initialized whatever we do
        unsafe {
            let inner_ptr = self
                .0
                .get()
                .as_ref()
                .expect("Config was not initialized before!");
            inner_ptr
        }
    }
}

unsafe impl Send for FhtConfig {}
unsafe impl Sync for FhtConfig {}

pub static CONFIG: LazyLock<FhtConfig> = LazyLock::new(|| {
    let inner = load_config().unwrap();
    FhtConfig(SyncUnsafeCell::new(inner))
});
pub static XDG_BASE_DIRECTORIES: LazyLock<BaseDirectories> =
    LazyLock::new(|| BaseDirectories::new().unwrap());

pub fn load_config() -> anyhow::Result<FhtConfigInner> {
    let config_file_path = XDG_BASE_DIRECTORIES
        .place_config_file("fht/compositor.ron")
        .context("Failed to get config file!")?;

    let reader = OpenOptions::new()
        .read(true)
        .write(false)
        .open(&config_file_path);
    let reader = match reader {
        Ok(reader) => reader,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // Create config file for user
            let mut file = File::create_new(&config_file_path).unwrap();
            writeln!(&mut file, "{}", DEFAULT_CONFIG).unwrap();
            OpenOptions::new()
                .read(true)
                .write(false)
                .open(&config_file_path)
                .unwrap()
        }
        Err(err) => {
            anyhow::bail!("Failed to open config file: {}", err)
        }
    };

    let config: FhtConfigInner = ron::de::from_reader(reader).context("Malformed config file!")?;
    Ok(config)
}

impl crate::state::State {
    #[profiling::function]
    pub fn reload_config(&mut self) {
        let new_config = match load_config() {
            Ok(config) => config,
            Err(err) => {
                self.fht.last_config_error = Some(err);
                return;
            }
        };

        if new_config.general.layouts.len() == 0 {
            self.fht.last_config_error =
                Some(anyhow::anyhow!("You have to specify at least one layout!"));
            return;
        }

        let old_config = CONFIG.clone();
        unsafe {
            *CONFIG.0.get() = new_config;
        }

        // the [`CursorThemeManager`] automatically checks for changes.
        self.fht.cursor_theme_manager.reload();
        self.fht
            .workspaces_mut()
            .for_each(|(_, wset)| wset.reload_config());

        let outputs = self.fht.outputs().cloned().collect::<Vec<_>>();
        for output in outputs {
            self.fht.output_resized(&output);
        }

        if CONFIG.input.keyboard != old_config.input.keyboard {
            if let Err(err) = self
                .fht
                .keyboard
                .clone()
                .set_xkb_config(self, CONFIG.input.keyboard.get_xkb_config())
            {
                error!(?err, "Failed to update keyboard xkb configuration!");
            }
        }

        for device in &mut self.fht.devices {
            let device_config = CONFIG
                .input
                .per_device
                .get(device.name())
                .or_else(|| CONFIG.input.per_device.get(device.sysname()));

            let mouse_config = device_config.map_or_else(|| &CONFIG.input.mouse, |cfg| &cfg.mouse);
            let keyboard_config =
                device_config.map_or_else(|| &CONFIG.input.keyboard, |cfg| &cfg.keyboard);
            let disabled = device_config.map_or(false, |cfg| cfg.disable);

            apply_libinput_settings(device, mouse_config, keyboard_config, disabled);
        }

        // I assume that if you have gone this far the config has reloaded sucessfully
        let _ = self.fht.last_config_error.take();
    }
}

pub fn apply_libinput_settings(
    device: &mut Device,
    mouse_config: &MouseConfig,
    _: &KeyboardConfig,
    disabled: bool,
) {
    let _ = device.config_send_events_set_mode(if disabled {
        SendEventsMode::DISABLED
    } else {
        SendEventsMode::ENABLED
    });

    let is_mouse = device.has_capability(DeviceCapability::Pointer);
    if is_mouse {
        let _ = device.config_left_handed_set(mouse_config.left_handed);
        let _ = device.config_accel_set_profile(mouse_config.acceleration_profile);
        let _ = device.config_accel_set_speed(mouse_config.acceleration_speed);
        let _ = device.config_middle_emulation_set_enabled(mouse_config.middle_button_emulation);

        // Based on mutter code, a touchpad should have more than one tap finger count.
        // Dont ask me why.
        let is_touchpad = device.config_tap_finger_count() > 0;
        if is_touchpad {
            let _ = device.config_tap_set_enabled(mouse_config.tap_to_click);
            let _ = device.config_dwt_set_enabled(mouse_config.disable_while_typing);
            let _ = device.config_scroll_set_natural_scroll_enabled(mouse_config.natural_scrolling);
            let _ = device.config_tap_set_button_map(mouse_config.tap_to_click_behaviour);
        }
    }
}
