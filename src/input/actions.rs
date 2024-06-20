use std::str::FromStr;

use serde::ser::SerializeSeq;
use serde::{Deserialize, Serialize, Serializer};
use smithay::backend::input::MouseButton;
use smithay::input::keyboard::{xkb, Keysym, ModifiersState};
use smithay::utils::Serial;

use crate::config::CONFIG;
use crate::shell::workspaces::tile::WorkspaceElement;
use crate::shell::{KeyboardFocusTarget, PointerFocusTarget};
use crate::state::State;
use crate::utils::geometry::{PointExt, RectCenterExt};
use crate::utils::output::OutputExt;

/// A list of modifiers you can use in a key pattern.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum Modifiers {
    ALT = 1,
    CTRL,
    SHIFT,
    SUPER,
}

impl<'lua> mlua::IntoLua<'lua> for Modifiers {
    fn into_lua(self, _: &'lua mlua::Lua) -> mlua::Result<mlua::Value<'lua>> {
        Ok(mlua::Value::Integer(self as i64))
    }
}

impl TryFrom<i64> for Modifiers {
    type Error = ();
    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::ALT),
            2 => Ok(Self::CTRL),
            3 => Ok(Self::SHIFT),
            4 => Ok(Self::SUPER),
            _ => Err(()),
        }
    }
}

impl FromStr for Modifiers {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "alt" | "A" => Ok(Self::ALT),
            "ctrl" | "C" => Ok(Self::CTRL),
            "shift" | "S" => Ok(Self::SHIFT),
            "super" | "M" => Ok(Self::SUPER),
            _ => Err(()),
        }
    }
}

impl<'lua> mlua::FromLua<'lua> for Modifiers {
    fn from_lua(value: mlua::Value<'lua>, _: &'lua mlua::Lua) -> mlua::Result<Self> {
        match value {
            mlua::Value::String(s) => {
                match Modifiers::from_str(s.to_str()?.to_lowercase().trim()) {
                    Ok(mod_) => Ok(mod_),
                    _ => Err(mlua::Error::FromLuaConversionError {
                        from: "string",
                        to: "Modifiers",
                        message: Some("No such modifier!".to_string()),
                    }),
                }
            }
            mlua::Value::Integer(int) => match Modifiers::try_from(int) {
                Ok(mod_) => Ok(mod_),
                _ => Err(mlua::Error::FromLuaConversionError {
                    from: "string",
                    to: "Modifiers",
                    message: Some("No such modifier!".to_string()),
                }),
            },
            _ => Err(mlua::Error::FromLuaConversionError {
                from: format!("{value:?}").leak(),
                to: "Modifiers",
                message: Some("Invalid value!".to_string()),
            }),
        }
    }
}

/// Custom adaptation of [`ModifiersState`] to allow for custom (de)serialization
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct FhtModifiersState {
    alt: bool,
    ctrl: bool,
    logo: bool,
    shift: bool,
}

impl From<ModifiersState> for FhtModifiersState {
    fn from(value: ModifiersState) -> Self {
        Self {
            alt: value.alt,
            ctrl: value.ctrl,
            logo: value.logo,
            shift: value.shift,
        }
    }
}

impl From<Vec<Modifiers>> for FhtModifiersState {
    fn from(value: Vec<Modifiers>) -> Self {
        Self {
            alt: value.contains(&Modifiers::ALT),
            ctrl: value.contains(&Modifiers::CTRL),
            logo: value.contains(&Modifiers::SUPER),
            shift: value.contains(&Modifiers::SHIFT),
        }
    }
}

impl Into<Vec<Modifiers>> for FhtModifiersState {
    fn into(self) -> Vec<Modifiers> {
        let mut vec = vec![];
        if self.alt {
            vec.push(Modifiers::ALT);
        }
        if self.ctrl {
            vec.push(Modifiers::CTRL);
        }
        if self.logo {
            vec.push(Modifiers::SUPER);
        }
        if self.shift {
            vec.push(Modifiers::SHIFT);
        }

        vec
    }
}

impl Serialize for FhtModifiersState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(None)?;

        if self.alt {
            seq.serialize_element(&Modifiers::ALT)?;
        }
        if self.ctrl {
            seq.serialize_element(&Modifiers::CTRL)?;
        }
        if self.logo {
            seq.serialize_element(&Modifiers::SUPER)?;
        }
        if self.shift {
            seq.serialize_element(&Modifiers::SHIFT)?;
        }

        seq.end()
    }
}

impl<'de> Deserialize<'de> for FhtModifiersState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mods = Vec::<Modifiers>::deserialize(deserializer)?;
        Ok(Self {
            alt: mods.contains(&Modifiers::ALT),
            ctrl: mods.contains(&Modifiers::CTRL),
            logo: mods.contains(&Modifiers::SUPER),
            shift: mods.contains(&Modifiers::SHIFT),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KeyAction {
    /// Quit the compositor
    Quit,

    /// Reload the compositor config.
    ReloadConfig,

    /// Run a given command, detaching its process from the compositor (basically the command won't
    /// be a child of the fht-compositor process)
    RunCommand(String),

    /// Select the next available layout on the current workspace.
    SelectNextLayout,

    /// Select the previous available layout on the current workspace.
    SelectPreviousLayout,

    /// Change the master width factor on the current workspace.
    ChangeMwfact(f32),

    /// Change the number of master clients on the current workspace.
    ChangeNmaster(i32),

    /// Change the cfact of the focused window.
    ChangeCfact(f32),

    /// Maximize the focused window on the current workspace.
    ///
    /// NOTE: You cant' have 2 maximized windows at a time.
    MaximizeFocusedWindow,

    /// Focus the next available window on the current workspace.
    FocusNextWindow,

    /// Focus the previous available window on the current workspace.
    FocusPreviousWindow,

    /// Swap the current and next window placements.
    SwapWithNextWindow,

    /// Swap the current and previous window placements.
    SwapWithPreviousWindow,

    /// Focus the next available output.
    FocusNextOutput,

    /// Focus the previous available output.
    FocusPreviousOutput,

    /// Close the currently focused window
    CloseFocusedWindow,

    /// Focus the workspace at a given index on the focused output.
    FocusWorkspace(usize),

    /// Send the focused window to the workspace at a given index on the focused output.
    SendFocusedWindowToWorkspace(usize),

    /// Do nothing.
    ///
    /// This is the same as disabling the key pattern for this action.
    None,
}

/// A key pattern.
///
/// For modifiers see [`Modifiers`]
///
/// ## Examples
///
/// ```rust,ignore
/// ([SUPER, SHIFT], "c")
/// ([SUPER, CTRL], "e")
/// ([SUPER], "k")
/// ```
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct KeyPattern(
    pub FhtModifiersState,
    #[serde(serialize_with = "ser::serialize_keysym")]
    #[serde(deserialize_with = "ser::deserialize_keysym")]
    pub Keysym,
);

impl<'lua> mlua::IntoLua<'lua> for KeyPattern {
    fn into_lua(self, lua: &'lua mlua::Lua) -> mlua::Result<mlua::Value<'lua>> {
        let table = lua.create_table()?;
        table.push(Into::<Vec<Modifiers>>::into(self.0))?;
        table.push(xkb::keysym_to_utf8(self.1))?;
        Ok(mlua::Value::Table(table))
    }
}

impl<'lua> mlua::FromLua<'lua> for KeyPattern {
    fn from_lua(value: mlua::Value<'lua>, _: &'lua mlua::Lua) -> mlua::Result<Self> {
        let values: Vec<_> = match value {
            mlua::Value::Table(table) => table
                .sequence_values::<String>()
                .filter_map(Result::ok)
                .collect(),
            mlua::Value::String(string) => string
                .to_str()?
                .split("-")
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
            _ => {
                return Err(mlua::Error::FromLuaConversionError {
                    from: value.type_name(),
                    to: "KeyPattern",
                    message: None,
                })
            }
        };

        // A user friendly way to parse key patterns.
        //
        // We get from the lua virtual machine a bunch of strings, in any order the user gives us.
        // The strings are meant to represent either modifiers or keys, for example we may get
        // 1. ["super", "alt", "k"] or ["alt", "k", "super"]
        // 2. ["k"]
        // 3. ["super", "j", "ctrl"]
        //
        // Note that you can precise as much modifiers as you want, but only one key, so, something
        // like this is invalid: ["super", "j", "k"]
        //
        // We also support emacs-like key patterns, like `M-S-c` (super shift c)
        let mut key = Option::<String>::None;
        let mut modifiers: Vec<Modifiers> = vec![];

        for value in values {
            let value = value.trim(); // cant to lowercase here since some mods are uppercase
            if let Some(mod_) = Modifiers::from_str(value).ok() {
                modifiers.push(mod_);
                continue;
            }

            // we have to make the value to lowercase since when we check for key patterns in the
            // input code, we turn the keysym to its lowercase form.
            if key.replace(value.to_lowercase()).is_some() {
                return Err(mlua::Error::FromLuaConversionError {
                    from: "table",
                    to: "KeyPattern",
                    message: Some("You can't specify two keys to bind!".to_string()),
                });
            }
        }

        let Some(key) = key else {
            return Err(mlua::Error::FromLuaConversionError {
                from: "table",
                to: "KeyPattern",
                message: Some("You have to specify atleast one key!".to_string()),
            });
        };
        let key = xkb::keysym_from_name(&key, xkb::KEYSYM_NO_FLAGS);

        Ok(Self(modifiers.into(), key))
    }
}

mod ser {

    use serde::de::Unexpected;
    use serde::{Deserialize, Deserializer, Serializer};
    use smithay::input::keyboard::xkb::{self, keysyms};
    use smithay::input::keyboard::Keysym;

    pub fn serialize_keysym<S: Serializer>(
        keysym: &Keysym,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(smithay::input::keyboard::xkb::keysym_get_name(*keysym).as_str())
    }

    pub fn deserialize_keysym<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Keysym, D::Error> {
        let name = String::deserialize(deserializer)?;

        // From the xkb rust crate itself, they recommend searching with `KEY_NO_FLAGS` then search
        // with `CASE_INSENSITIVE` to be more precise in your search, since
        // `KEYSYM_CASE_INSENSITIVE` will always return the lowercase letter
        match xkb::keysym_from_name(&name, xkb::KEYSYM_NO_FLAGS).raw() {
            keysyms::KEY_NoSymbol => {
                match xkb::keysym_from_name(&name, xkb::KEYSYM_CASE_INSENSITIVE).raw() {
                    keysyms::KEY_NoSymbol => Err(<D::Error as serde::de::Error>::invalid_value(
                        Unexpected::Str(&name),
                        &"Invalid keysym!",
                    )),
                    keysym => Ok(keysym.into()),
                }
            }
            keysym => Ok(keysym.into()),
        }
    }
}

impl State {
    #[profiling::function]
    pub fn process_key_action(&mut self, action: KeyAction) {
        let Some(ref output) = self.fht.focus_state.output.clone() else {
            return;
        };
        let current_focus = self.fht.focus_state.focus_target.clone();
        let wset = self.fht.wset_mut_for(output);
        let active = wset.active_mut();

        match action {
            KeyAction::Quit => self
                .fht
                .stop
                .store(true, std::sync::atomic::Ordering::SeqCst),
            KeyAction::ReloadConfig => self.reload_config(),
            KeyAction::RunCommand(cmd) => crate::utils::spawn(cmd),
            KeyAction::SelectNextLayout => active.select_next_layout(),
            KeyAction::SelectPreviousLayout => active.select_previous_layout(),
            KeyAction::ChangeMwfact(delta) => active.change_mwfact(delta),
            KeyAction::ChangeNmaster(delta) => active.change_nmaster(delta),
            KeyAction::ChangeCfact(delta) => {
                let mut arrange = false;
                if let Some(tile) = active.focused_tile_mut() {
                    tile.cfact += delta;
                    arrange = true;
                }
                if arrange {
                    active.arrange_tiles();
                }
            }
            KeyAction::MaximizeFocusedWindow => {
                if let Some(window) = active.focused().cloned() {
                    let new_maximized = !window.maximized();
                    window.set_maximized(new_maximized);
                    active.arrange_tiles();
                }
            }
            KeyAction::FocusNextWindow => {
                let new_focus = active.focus_next_element().cloned();
                if let Some(window) = new_focus {
                    if CONFIG.general.cursor_warps {
                        let center = active.element_geometry(&window).unwrap().center();
                        self.move_pointer(center.to_f64())
                    }
                    self.set_focus_target(Some(window.into()));
                }
            }
            KeyAction::FocusPreviousWindow => {
                let new_focus = active.focus_previous_element().cloned();
                if let Some(window) = new_focus {
                    if CONFIG.general.cursor_warps {
                        let center = active.element_geometry(&window).unwrap().center();
                        self.move_pointer(center.to_f64())
                    }
                    self.set_focus_target(Some(window.into()));
                }
            }
            KeyAction::SwapWithNextWindow => {
                active.swap_with_next_element();
                if let Some(window) = active.focused().cloned() {
                    if CONFIG.general.cursor_warps {
                        let center = active.element_geometry(&window).unwrap().center();
                        self.move_pointer(center.to_f64())
                    }
                    self.set_focus_target(Some(window.into()));
                }
            }
            KeyAction::SwapWithPreviousWindow => {
                active.swap_with_previous_element();
                if let Some(window) = active.focused().cloned() {
                    if CONFIG.general.cursor_warps {
                        let center = active.element_geometry(&window).unwrap().center();
                        self.move_pointer(center.to_f64())
                    }
                    self.set_focus_target(Some(window.into()));
                }
            }
            KeyAction::FocusNextOutput => {
                let outputs_len = self.fht.workspaces.len();
                if outputs_len < 2 {
                    return;
                }

                let current_output_idx = self
                    .fht
                    .outputs()
                    .position(|o| o == output)
                    .expect("Focused output is not registered");

                let mut next_output_idx = current_output_idx + 1;
                if next_output_idx == outputs_len {
                    next_output_idx = 0;
                }

                let output = self
                    .fht
                    .outputs()
                    .skip(next_output_idx)
                    .next()
                    .unwrap()
                    .clone();
                if CONFIG.general.cursor_warps {
                    let center = output.geometry().center();
                    self.move_pointer(center.to_f64());
                }
                self.fht.focus_state.output.replace(output).unwrap();
            }
            KeyAction::FocusPreviousOutput => {
                let outputs_len = self.fht.workspaces.len();
                if outputs_len < 2 {
                    return;
                }

                let current_output_idx = self
                    .fht
                    .outputs()
                    .position(|o| o == output)
                    .expect("Focused output is not registered");

                let next_output_idx = match current_output_idx.checked_sub(1) {
                    Some(idx) => idx,
                    None => outputs_len - 1,
                };

                let output = self
                    .fht
                    .outputs()
                    .skip(next_output_idx)
                    .next()
                    .unwrap()
                    .clone();
                if CONFIG.general.cursor_warps {
                    let center = output.geometry().center();
                    self.move_pointer(center.to_f64());
                }
                self.fht.focus_state.output.replace(output).unwrap();
            }
            KeyAction::CloseFocusedWindow => {
                if let Some(KeyboardFocusTarget::Window(window)) = current_focus {
                    window.toplevel().unwrap().send_close();
                }
                self.set_focus_target(None); // reset focus
            }
            KeyAction::FocusWorkspace(idx) => {
                if let Some(window) = wset.set_active_idx(idx, true) {
                    self.set_focus_target(Some(window.into()));
                };
            }
            KeyAction::SendFocusedWindowToWorkspace(idx) => {
                let Some(window) = active.focused().cloned() else {
                    return;
                };
                let tile = active.remove_tile(&window).unwrap();
                let new_focus = active.focused().cloned();
                let idx = idx.clamp(0, 9);
                wset.workspaces[idx].insert_tile(tile);

                if let Some(window) = new_focus {
                    self.set_focus_target(Some(window.into()));
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum FhtMouseButton {
    Left,
    Middle,
    Right,
    Forward,
    Back,
}

impl From<MouseButton> for FhtMouseButton {
    fn from(value: MouseButton) -> Self {
        match value {
            MouseButton::Left => Self::Left,
            MouseButton::Middle => Self::Middle,
            MouseButton::Right => Self::Right,
            MouseButton::Forward => Self::Forward,
            MouseButton::Back => Self::Back,
            _ => Self::Left,
        }
    }
}

impl Into<MouseButton> for FhtMouseButton {
    fn into(self) -> MouseButton {
        match self {
            Self::Left => MouseButton::Left,
            Self::Middle => MouseButton::Middle,
            Self::Right => MouseButton::Right,
            Self::Forward => MouseButton::Forward,
            Self::Back => MouseButton::Back,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MouseAction {
    /// Move the window under the cursor
    MoveTile,
}

/// A mouse pattern.
///
/// For modifiers see [`Modifiers`]
///
/// ```rust,ignore
/// ([SUPER], LMB)
/// ([SUPER], RMB)
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct MousePattern(pub FhtModifiersState, pub FhtMouseButton);

impl State {
    #[profiling::function]
    pub fn process_mouse_action(&mut self, action: MouseAction, serial: Serial) {
        let pointer_loc = self.fht.pointer.current_location().as_global();

        match action {
            MouseAction::MoveTile => {
                if let Some((PointerFocusTarget::Window(window), _)) =
                    self.fht.focus_target_under(pointer_loc)
                {
                    self.fht
                        .loop_handle
                        .insert_idle(move |state| state.handle_move_request(window, serial));
                }
            }
        }
    }
}
