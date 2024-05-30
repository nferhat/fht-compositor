mod types;

use fht_config::{Config, ConfigWrapper};
use smithay::reexports::calloop::{self, LoopHandle, RegistrationToken};
use smithay::reexports::input::{Device, DeviceCapability, SendEventsMode};

#[allow(unused_imports)]
pub use self::types::{
    AnimationConfig, BorderConfig, ColorConfig, CompositorConfig, CursorConfig, GeneralConfig,
    InputConfig, InsertWindowStrategy, KeyboardConfig, MouseConfig, PerDeviceInputConfig,
    WindowMapSettings, WindowRulePattern, WorkspaceSwitchAnimationConfig,
    WorkspaceSwitchAnimationDirection,
};
use crate::state::State;

pub static CONFIG: ConfigWrapper<CompositorConfig> = ConfigWrapper::new();

pub fn init_config_file_watcher(
    loop_handle: &LoopHandle<'static, State>,
) -> anyhow::Result<RegistrationToken> {
    // Unit as a dumb message for "reload config"
    let (sender, channel) = calloop::channel::channel::<()>();
    let watcher_token = loop_handle
        .insert_source(channel, |event, (), state| {
            let calloop::channel::Event::Msg(()) = event else {
                return;
            };
            state.reload_config();
        })
        .map_err(|err| anyhow::anyhow!("Failed to insert config file watcher source! {err}"))?;

    async_std::task::spawn(async move {
        let path = CompositorConfig::get_path();
        let mut last_mtime = path.metadata().and_then(|md| md.modified()).ok();

        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if let Some(new_mtime) = path
                .metadata()
                .and_then(|md| md.modified())
                .ok()
                .filter(|mt| Some(mt) != last_mtime.as_ref())
            {
                trace!(?new_mtime, "Config file change detected.");
                last_mtime = Some(new_mtime);
                if let Err(err) = sender.send(()) {
                    warn!(?err, "Failed to notify config file change!");
                };
            }
        }
    });

    Ok(watcher_token)
}

impl State {
    #[profiling::function]
    pub fn reload_config(&mut self) {
        let new_config = match CompositorConfig::load() {
            Ok(config) => config,
            Err(err) => {
                self.fht.last_config_error = Some(anyhow::anyhow!(err));
                return;
            }
        };

        if new_config.general.layouts.len() == 0 {
            self.fht.last_config_error =
                Some(anyhow::anyhow!("You have to specify at least one layout!"));
            return;
        }

        let old_config = CONFIG.clone();
        CONFIG.set(new_config);

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
