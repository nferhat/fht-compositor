use serde::ser::SerializeSeq;
use serde::{Deserialize, Serialize, Serializer};
use smithay::backend::input::MouseButton;
use smithay::input::keyboard::{Keysym, ModifiersState};
use smithay::utils::Serial;

use crate::config::CONFIG;
use crate::shell::FocusTarget;
use crate::state::State;
use crate::utils::output::OutputExt;

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

    /// Toggle floating mode for the focused window.
    ToggleFloating,

    /// Pins the focused window, making it show above all the workspaces of an output, no matter
    /// which workspace it is
    ///
    /// Works only if the window you are trying to pin is floating.
    ///
    /// NOTE: I have to implement pinning before lmao
    PinFocusedWindow,

    /// Move the focused window to the center of it's monitor.
    ///
    /// Works only if the window you are trying to ping is floating.
    CenterFocusedWindow,

    /// Fullscreens the focused window on the current workspace
    ///
    /// NOTE: You can't have 2 fullscreened windows at a time.
    FullscreenFocusedWindow,

    /// Maximize the focused window on the current workspace.
    ///
    /// NOTE: You cant' have 2 maximized windows at a time.
    ToggleMaximizeOnFocusedWindow,

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

/// A list of modifiers you can use in a key pattern.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum Modifiers {
    ALT,
    CTRL,
    SHIFT,
    SUPER,
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct KeyPattern(
    pub FhtModifiersState,
    #[serde(serialize_with = "ser::serialize_keysym")]
    #[serde(deserialize_with = "ser::deserialize_keysym")]
    pub Keysym,
);

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
            KeyAction::ToggleFloating => {
                if let Some(window) = active.focused().cloned() {
                    let new_tiled = !window.is_tiled();
                    window.set_tiled(new_tiled);
                    active.raise_window(&window);
                    active.refresh_window_geometries();
                }
            }
            KeyAction::PinFocusedWindow => {
                todo!("PinFocusedWindow support");
            }
            KeyAction::CenterFocusedWindow => {
                if let Some(window) = active.focused().filter(|w| !w.is_tiled()).cloned() {
                    let mut geo = window.global_geometry();
                    let output_geo = output.geometry();
                    geo.loc = output_geo.loc + output_geo.size.downscale(2).to_point();
                    geo.loc -= geo.size.downscale(2).to_point();
                    window.set_geometry(geo);
                }
            }
            KeyAction::FullscreenFocusedWindow => {
                if let Some(window) = active.focused().cloned() {
                    window.set_fullscreen(true, None);
                    active.refresh_window_geometries();
                }
            }
            KeyAction::ToggleMaximizeOnFocusedWindow => {
                if let Some(window) = active.focused().cloned() {
                    let new_maximized = !window.is_maximized();
                    window.set_maximized(new_maximized);
                    active.refresh_window_geometries();
                }
            }
            KeyAction::FocusNextWindow => {
                let new_focus = active.focus_next_window().cloned();
                if let Some(window) = new_focus {
                    if CONFIG.general.warp_window_on_focus {
                        let window_geo = window.global_geometry();
                        let center = window_geo.loc + window_geo.size.downscale(2).to_point();
                        self.move_pointer(center.to_f64())
                    }
                    self.fht.focus_state.focus_target = Some(window.into());
                }
            }
            KeyAction::FocusPreviousWindow => {
                let new_focus = active.focus_previous_window().cloned();
                if let Some(window) = new_focus {
                    if CONFIG.general.warp_window_on_focus {
                        let window_geo = window.global_geometry();
                        let center = window_geo.loc + window_geo.size.downscale(2).to_point();
                        self.fht.focus_state.focus_target = Some(window.into());
                        self.move_pointer(center.to_f64())
                    }
                }
            }
            KeyAction::SwapWithNextWindow => {
                active.swap_with_next_window();
                if let Some(window) = active.focused().cloned() {
                    if CONFIG.general.warp_window_on_focus {
                        let window_geo = window.global_geometry();
                        let center = window_geo.loc + window_geo.size.downscale(2).to_point();
                        self.move_pointer(center.to_f64())
                    }
                    self.fht.focus_state.focus_target = Some(window.into());
                }
            }
            KeyAction::SwapWithPreviousWindow => {
                active.swap_with_previous_window();
                if let Some(window) = active.focused().cloned() {
                    if CONFIG.general.warp_window_on_focus {
                        let window_geo = window.global_geometry();
                        let center = window_geo.loc + window_geo.size.downscale(2).to_point();
                        self.move_pointer(center.to_f64())
                    }
                    self.fht.focus_state.focus_target = Some(window.into());
                }
            }
            KeyAction::FocusNextOutput => {
                // TODO: yes
            }
            KeyAction::FocusPreviousOutput => {
                // TODO: yes
            }
            KeyAction::CloseFocusedWindow => {
                if let Some(window) = active.focused() {
                    window.close()
                }
                active.refresh();
                if let Some(window) = active.focused().cloned() {
                    self.fht.focus_state.focus_target = Some(window.into());
                }
            }
            KeyAction::FocusWorkspace(idx) => {
                wset.set_active_idx(idx);
                let new_active = wset.active();
                if let Some(window) = new_active.focused().cloned() {
                    self.fht.focus_state.focus_target = Some(window.into());
                }
            }
            KeyAction::SendFocusedWindowToWorkspace(idx) => {
                dbg!("hi");
                let Some(window) = active.focused().cloned() else {
                    return;
                };
                let window = active.remove_window(&window).unwrap();
                let idx = idx.clamp(0, 9);
                wset.workspaces[idx].insert_window(window);
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
    MoveWindow {
        /// Should this move window action affect only floating windows.
        ///
        /// In other terms, if this is true, only floating windows will be affected by the grab,
        /// otherwise, every window, including tiled ones (that will get untiled when the action is
        /// active) will be affected by this action.
        floating_only: bool,
    },
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
        let pointer_loc = self.fht.pointer.current_location();

        match action {
            MouseAction::MoveWindow { floating_only } => {
                if let Some((FocusTarget::Window(window), _)) =
                    self.fht.focus_target_under(pointer_loc)
                {
                    if window.is_tiled() && floating_only {
                        return;
                    }
                    self.fht.loop_handle.insert_idle(move |state| {
                        state.handle_move_request(window, serial);
                    });
                }
            }
        }
    }
}
