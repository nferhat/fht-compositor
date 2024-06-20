use std::rc::Rc;

use indexmap::IndexMap;
use smithay::reexports::calloop;

use super::LuaMessage;
use crate::input::KeyPattern;

/// A binding manager.
///
/// Its able to register and unregister keymaps and mouse maps from the main compositor state.
pub struct BindManager {
    // NOTE: Using an Rc to have cheap copies when using the `key_binds` metamethod.
    pub(super) keybinds: IndexMap<KeyPattern, Rc<KeyBind>>,
    to_compositor: calloop::channel::Sender<LuaMessage>,
}

/// A single action to trigger on a [`KeyPattern`]
pub struct KeyBind {
    /// The registry key that points to the callback of this bind.
    pub registry_key: mlua::RegistryKey,
    /// The (optional) group name of this key bind.
    ///
    /// Something general, along of the lines of: "Focus", "Output", "Workspaces", etc.
    pub group: String,
    /// The (optional) description of this key bind.
    pub description: String,
}

impl mlua::UserData for KeyBind {
    fn add_fields<'lua, F: mlua::UserDataFields<'lua, Self>>(fields: &mut F) {
        fields.add_field_method_get("group", |_, kb| Ok(kb.group.clone()));
        fields.add_field_method_get("description", |_, kb| Ok(kb.description.clone()));
    }

    fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("run_callback", |lua, kb, ()| {
            let callback: mlua::Function = lua.registry_value(&kb.registry_key)?;
            let _: () = callback.call(())?;
            Ok(())
        })
    }
}

impl mlua::UserData for BindManager {
    fn add_fields<'lua, F: mlua::UserDataFields<'lua, Self>>(_fields: &mut F) {}

    fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method_mut(
            "bind_key",
            |lua, bm, (key_pattern, callback, properties)| {
                let key_pattern: KeyPattern = key_pattern;
                let callback: mlua::Function = callback;
                let properties: Option<mlua::Table> = properties;

                // We store the callback only on the lua side. When rust sends us any kind of
                // keybind, we do the filter (and see whether we have a callback to run or not)
                // here, on the lua thread.
                //
                // We still have to inform the compositor about the keys we bind or not, to avoid
                // sending bound key events to clients.
                let registry_key = lua.create_registry_value(callback)?;
                let group = properties
                    .as_ref()
                    .and_then(|props| props.get::<_, String>("group".to_string()).ok())
                    .unwrap_or_default();
                let description = properties
                    .as_ref()
                    .and_then(|props| props.get::<_, String>("description".to_string()).ok())
                    .unwrap_or_default();

                let key_bind = Rc::new(KeyBind {
                    registry_key,
                    group,
                    description,
                });

                let _ = bm.keybinds.insert(key_pattern, key_bind);
                bm.to_compositor
                    .send(LuaMessage::NewKeybind { key_pattern })
                    .unwrap();

                Ok(())
            },
        );

        methods.add_method_mut("unbind_key", |lua, bm, key_pattern: KeyPattern| {
            let Some(key_bind) = bm.keybinds.swap_remove(&key_pattern) else {
                // Eh, some weird stuff the user has been doing, No need to error out for this.
                return Ok(());
            };
            // SAFETY: We are the only ones who own this Rc<RegistryKey>, using it to have
            // cheap copies of the key bind definition.
            let key_bind = Rc::into_inner(key_bind).unwrap();

            lua.remove_registry_value(key_bind.registry_key).unwrap();
            bm.to_compositor
                .send(LuaMessage::RemoveKeybind { key_pattern })
                .unwrap();

            Ok(())
        });

        methods.add_method("key_binds", |lua, bm, ()| {
            // TODO: I fucking hate this, allocation for nothing, because of 'static trait bounds.
            lua.create_table_from(bm.keybinds.clone())
        })
    }
}

impl BindManager {
    /// Create a new binding manager.
    pub fn new(to_compositor: calloop::channel::Sender<LuaMessage>) -> Self {
        Self {
            keybinds: IndexMap::new(),
            to_compositor,
        }
    }
}
