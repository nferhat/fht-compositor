//! The lua integration for fht-compositor.
//!
//! Lua is the main scripting language used by the compositor to configure and run its activities.
//! It evaluates by default `$XDG_CONFIG_HOME/fht/compositor.lua` as the default configuration
//! script.
//!
//! In each lua environment, the compositor provides you with a `fht` global table, with functions
//! and objects to hook into compositor activity, or listen to signals, and other utilities
//! allowing you to write configuration or scripts.
//!
//! TODO:
//! - Add output manager
//!     - Create output object
//!     - Create workspace object
//! - Add rule manager
//!     - Create window rule object
//! -

use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::{Arc, LazyLock, RwLock};

use async_std::path::PathBuf;
use indexmap::IndexMap;
use smithay::reexports::calloop;
use smithay::reexports::rustix::path::Arg;

use self::api::BindManager;
use self::signal::Signal;
use crate::input::KeyPattern;

pub mod api;
pub mod signal;

pub static CONFIG_PATH: LazyLock<String> = LazyLock::new(|| {
    xdg::BaseDirectories::new()
        .expect("Not in a XDG environment!")
        .get_config_file("fht/compositor.lua")
        .to_string_lossy()
        .to_string()
});

type Signals = IndexMap<Signal, Vec<mlua::RegistryKey>>;

/// A full lua virtual machine with custom objects and globals.
struct LuaVM {
    inner: mlua::Lua,
    bind_manager: Arc<RwLock<BindManager>>,
    signals: Arc<RwLock<Signals>>,
    to_compositor: calloop::channel::Sender<LuaMessage>,
}

impl LuaVM {
    /// Create a new instance of the lua virtual machine.
    fn new(to_compositor: calloop::channel::Sender<LuaMessage>) -> Self {
        let vm = Self {
            inner: mlua::Lua::new(),
            bind_manager: Arc::new(RwLock::new(api::BindManager::new(to_compositor.clone()))),
            signals: Arc::new(RwLock::new(IndexMap::new())),
            to_compositor,
        };
        let lua = &vm.inner;
        let globals = lua.globals();

        {
            let search_path = PathBuf::from_str(&CONFIG_PATH).unwrap();
            let search_path = search_path.parent().unwrap();
            let search_path = search_path.to_str().unwrap();

            // Copied from AwesomeWM (luaa.c, line 1039, add_to_search_path)
            let package: mlua::Table = globals.get("package").unwrap();
            let mut package_path: String = package.get("path").unwrap();
            package_path.push_str(&format!(";{search_path}/?.lua"));
            package_path.push_str(&format!(";{search_path}/?/init.lua"));
            package.set("path", package_path).unwrap();

            let mut package_cpath: String = package.get("cpath").unwrap();
            package_cpath.push_str(&format!(";{search_path}/?.so"));
            package.set("cpath", package_cpath).unwrap();
        }

        let fht = lua.create_table().unwrap();

        // Logging functions
        let info_function = lua
            .create_function(|_, msg: String| {
                info!("Lua: {msg}");
                Ok(())
            })
            .unwrap();
        fht.set("info", info_function).unwrap();

        let warn_function = lua
            .create_function(|_, msg: String| {
                warn!("Lua: {msg}");
                Ok(())
            })
            .unwrap();
        fht.set("warn", warn_function).unwrap();

        let error_function = lua
            .create_function(|_, msg: String| {
                error!("Lua: {msg}");
                Ok(())
            })
            .unwrap();
        fht.set("error", error_function).unwrap();

        let debug_function = lua
            .create_function(|_, msg: String| {
                debug!("Lua: {msg}");
                Ok(())
            })
            .unwrap();
        fht.set("debug", debug_function).unwrap();

        let register_callback = {
            let signals = Arc::clone(&vm.signals);

            lua.create_function(move |lua, (name, callback): (String, mlua::Function)| {
                let Ok(signal) = name.parse::<Signal>() else {
                    return Err(mlua::Error::FromLuaConversionError {
                        from: name.leak(),
                        to: "Signal",
                        message: Some("No such signal".to_string()),
                    });
                };

                let registry_key = lua.create_registry_value(callback)?;
                let callback_id = get_registry_id(&registry_key);

                let mut signals = signals.write().unwrap();
                let signals = signals.entry(signal).or_insert_with(Default::default);
                signals.push(registry_key);

                Ok(callback_id)
            })
            .unwrap()
        };
        fht.set("register_callback", register_callback).unwrap();

        let unregister_callback = {
            let signals = Arc::clone(&vm.signals);

            lua.create_function(move |_, (name, callback_id): (String, u64)| {
                let Ok(signal) = name.parse::<Signal>() else {
                    return Err(mlua::Error::FromLuaConversionError {
                        from: name.leak(),
                        to: "Signal",
                        message: Some("No such signal".to_string()),
                    });
                };

                let mut signals = signals.write().unwrap();
                let signals = signals.entry(signal).or_insert_with(Default::default);
                signals.retain(|key| get_registry_id(key) != callback_id);
                Ok(())
            })
            .unwrap()
        };
        fht.set("unregister_callback", unregister_callback).unwrap();

        // Binding manager, used to manage keybinds and mousebinds
        fht.set("bind_manager", Arc::clone(&vm.bind_manager))
            .unwrap();

        globals.set("fht", fht).unwrap();
        drop(globals);

        vm
    }

    /// Reload the configuration.
    fn reload_configuration(&self) {
        self.signals.write().unwrap().clear();
        let mut bind_manager = self.bind_manager.write().unwrap();
        for (key_pattern, _) in bind_manager.keybinds.drain(..) {
            self.to_compositor.send(LuaMessage::RemoveKeybind { key_pattern }).unwrap();
        }

        let config_contents = match std::fs::read_to_string(CONFIG_PATH.as_str()) {
            Ok(c) => c,
            Err(err) => {
                error!(?err, "Failed to open configuration file!");
                std::process::exit(1);
            }
        };
        let _: () = self
            .inner
            .load(config_contents)
            .eval()
            .expect("Failed to execute your configuration!");
    }

    fn handle_compositor_message(&self, msg: CompositorMessage) {
        match msg {
            CompositorMessage::Signal(signal) => {
                let signals = self.signals.read().unwrap();
                let Some(signals) = signals.get(&signal) else {
                    return;
                };
                for signal in signals {
                    let callback: mlua::Function = self
                        .inner
                        .registry_value(&signal)
                        .expect("Signals should always be registered!");
                    let _: Result<(), _> = callback.call(727); // funny easter egg
                }
            }
            CompositorMessage::KeyPatternPressed { key_pattern } => {
                let bind_manager = self.bind_manager.read().unwrap();
                let Some(key_bind) = bind_manager.keybinds.get(&key_pattern) else {
                    return;
                };
                let callback: mlua::Function = self
                    .inner
                    .registry_value(&key_bind.registry_key)
                    .expect("Keybind callbacks should always be registered!");
                let _: Result<(), _> = callback.call(());
            }
            CompositorMessage::ReloadConfig => {
                self.reload_configuration()
            }
        }
    }
}

/// Start the lua virtual machine.
///
/// You will not be able to access it. You will get a channel (receiver from lua) and a sender (to
/// lua). The virtual machine lives on another thread to not block main compositor activity.
pub fn start() -> (
    calloop::channel::Channel<LuaMessage>,
    std::sync::mpsc::Sender<CompositorMessage>,
) {
    let (to_compositor, from_lua) = calloop::channel::channel();
    let (to_lua, from_compositor) = std::sync::mpsc::channel();

    let _join_handle = std::thread::Builder::new()
        .name("lua_vm".to_string())
        .spawn(move || {
            let lua = LuaVM::new(to_compositor);

            let config_contents = match std::fs::read_to_string(CONFIG_PATH.as_str()) {
                Ok(c) => c,
                Err(err) => {
                    error!(?err, "Failed to open configuration file!");
                    std::process::exit(1);
                }
            };
            let _: () = lua
                .inner
                .load(config_contents)
                .eval()
                .expect("Failed to execute your configuration!");

            // NOTE: Need to capture outside the loop so that it doesn't drop after the first
            // message received (so when the first iteration finishes)
            let from_compositor = from_compositor;
            while let Some(msg) = from_compositor.recv().ok() {
                lua.handle_compositor_message(msg);
            }
        })
        .expect("Failed to start lua virtual machine thread!");

    (from_lua, to_lua)
}

/// A message from lua
#[derive(Debug)]
pub enum LuaMessage {
    /// A new keybind has been registered
    NewKeybind {
        /// The key pattern associated with this keybind.
        key_pattern: KeyPattern,
    },

    /// A new keybind has been unregistered
    RemoveKeybind {
        /// The key pattern associated with this keybind.
        key_pattern: KeyPattern,
    },
}

/// A message from the compositor.
#[derive(Debug)]
pub enum CompositorMessage {
    /// Compositor emitted a signal to the lua virtual machine.
    Signal(Signal),

    /// A bound [`KeyPattern`] has been pressed.
    KeyPatternPressed {
        /// The pressed [`KeyPattern`]
        ///
        /// The compositor state assures that this key pattern has a callback associated to it.
        key_pattern: KeyPattern,
    },

    /// The user requested to reload the lua configuration.
    ReloadConfig,
}

/// Get the registry_id of a [`RegistryKey`](mlua::RegistryKey)
/// TODO: Maybe not use a hasher each time?
fn get_registry_id(key: &mlua::RegistryKey) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}
