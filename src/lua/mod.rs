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
//! - Add keybind manager
//! - Add output manager
//!     - Create output object
//!     - Create workspace object
//! - Add rule manager
//!     - Create window rule object
//! -

use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock, Mutex};

use indexmap::IndexMap;
use smithay::reexports::calloop;
use smithay::reexports::rustix::path::Arg;

use self::signal::Signal;

pub mod signal;
pub mod api;

pub static CONFIG_PATH: LazyLock<String> = LazyLock::new(|| {
    xdg::BaseDirectories::new()
        .expect("Not in a XDG environment!")
        .get_config_file("fht/compositor.lua")
        .to_string_lossy()
        .to_string()
});

type Signals = IndexMap<Signal, Vec<mlua::RegistryKey>>;

/// Start the lua virtual machine.
///
/// You will not be able to access it. You will get a channel (receiver from lua) and a sender (to
/// lua). The virtual machine lives on another thread to not block main compositor activity.
pub fn start() -> (
    calloop::channel::Channel<LuaMessage>,
    async_std::channel::Sender<CompositorMessage>,
) {
    let (to_compositor, from_lua) = calloop::channel::channel();
    let (to_lua, from_compositor) = async_std::channel::unbounded();

    let _join_handle = async_std::task::spawn(async move {
        let lua = Box::pin(mlua::Lua::new());
        let signals = Arc::new(Mutex::new(IndexMap::new()));
        self::setup_globals(&lua, Arc::clone(&signals));

        let config_contents = match std::fs::read_to_string(CONFIG_PATH.as_str()) {
            Ok(c) => c,
            Err(err) => {
                error!(?err, "Failed to open configuration file!");
                std::process::exit(1);
            }
        };
        let _: () = lua
            .load(config_contents)
            .eval()
            .expect("Failed to execute your configuration!");

        // NOTE: Need to capture outside the loop so that it doesn't drop after the first
        // message received (so when the first iteration finishes)
        let from_compositor = from_compositor;
        while let Some(msg) = from_compositor.recv().await.ok() {
            let signals = Arc::clone(&signals);
            self::handle_compositor_message(msg, &lua, signals)
        }
    });

    (from_lua, to_lua)
}

fn setup_globals(lua: &mlua::Lua, signals: Arc<Mutex<Signals>>) {
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
        let signals = Arc::clone(&signals);

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

            let mut signals = signals.lock().unwrap();
            let signals = signals.entry(signal).or_insert_with(Default::default);
            signals.push(registry_key);

            Ok(callback_id)
        })
        .unwrap()
    };
    fht.set("register_callback", register_callback).unwrap();

    let unregister_callback = {
        let signals = Arc::clone(&signals);

        lua.create_function(move |_, (name, callback_id): (String, u64)| {
            let Ok(signal) = name.parse::<Signal>() else {
                return Err(mlua::Error::FromLuaConversionError {
                    from: name.leak(),
                    to: "Signal",
                    message: Some("No such signal".to_string()),
                });
            };

            let mut signals = signals.lock().unwrap();
            let signals = signals.entry(signal).or_insert_with(Default::default);
            signals.retain(|key| get_registry_id(key) != callback_id);
            Ok(())
        })
        .unwrap()
    };
    fht.set("unregister_callback", unregister_callback).unwrap();

    lua.globals().set("fht", fht).unwrap();
}

/// A message from lua
#[derive(Debug)]
pub enum LuaMessage {}

/// A message from the compositor.
#[derive(Debug)]
pub enum CompositorMessage {
    /// Compositor emitted a signal to the lua virtual machine.
    Signal(Signal),
}

fn handle_compositor_message(
    msg: CompositorMessage,
    lua: &mlua::Lua,
    signals: Arc<Mutex<Signals>>,
) {
    match msg {
        CompositorMessage::Signal(signal) => {
            let signals = signals.lock().unwrap();
            let Some(signals) = signals.get(&signal) else {
                return;
            };
            for signal in signals {
                let callback: mlua::Function = lua
                    .registry_value(&signal)
                    .expect("Signals should always be registered!");
                let _: Result<(), _> = callback.call(727); // funny easter egg
            }
        }
    }
}

/// Get the registry_id of a [`RegistryKey`](mlua::RegistryKey)
/// TODO: Maybe not use a hasher each time?
fn get_registry_id(key: &mlua::RegistryKey) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}
