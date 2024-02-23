// This file is a light wrapper around libxkbcommon, see the other file for usage

use egui::{Key, Modifiers, PointerButton};
use smithay::{
    backend::input::MouseButton,
    input::keyboard::{Keysym as KeysymU32, ModifiersState},
};
use xkbcommon::xkb;
pub use xkbcommon::xkb::{keysyms, Keycode, Keysym};

use std::convert::TryFrom;

pub struct KbdInternal {
    keymap: xkb::Keymap,
    state: xkb::State,
}
// SAFETY: This is OK, because all parts of xkb will remain on the same thread
unsafe impl Send for KbdInternal {}

// focus_hook does not implement debug, so we have to impl Debug manually
impl std::fmt::Debug for KbdInternal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KbdInternal")
            .field("keymap", &self.keymap.get_raw_ptr())
            .field("state", &self.state.get_raw_ptr())
            .finish()
    }
}

impl KbdInternal {
    pub fn new() -> Option<KbdInternal> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            "",
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )?;
        let state = xkb::State::new(&keymap);
        Some(KbdInternal { keymap, state })
    }

    // return true if modifier state has changed
    pub fn key_input(&mut self, keycode: u32, pressed: bool) {
        let direction = match pressed {
            true => xkb::KeyDirection::Down,
            false => xkb::KeyDirection::Up,
        };

        // update state (keycode is already offset by 8)
        self.state.update_key(Keycode::new(keycode), direction);
    }

    pub fn get_utf8(&self, keycode: u32) -> String {
        self.state.key_get_utf8(Keycode::new(keycode))
    }
}

/// Converts a set of raw keycodes into [`egui::Key`], if possible.
pub fn convert_key(keys: impl Iterator<Item = KeysymU32>) -> Option<Key> {
    for sym in keys {
        if let Ok(key) = KeysymConv(sym).try_into() {
            return Some(key);
        }
    }
    None
}

pub struct KeysymConv(pub KeysymU32);

impl TryFrom<KeysymConv> for Key {
    type Error = ();

    fn try_from(sym: KeysymConv) -> Result<Key, ()> {
        use egui::Key::*;

        #[allow(non_upper_case_globals)]
        Ok(match sym.0 {
            Keysym::Down => ArrowDown,
            Keysym::Left => ArrowLeft,
            Keysym::Right => ArrowRight,
            Keysym::Up => ArrowUp,
            Keysym::Escape => Escape,
            Keysym::Tab => Tab,
            Keysym::BackSpace => Backspace,
            Keysym::Return => Enter,
            Keysym::space => Space,
            Keysym::Insert => Insert,
            Keysym::Delete => Delete,
            Keysym::Home => Home,
            Keysym::End => End,
            Keysym::Page_Up => PageUp,
            Keysym::Page_Down => PageDown,
            Keysym::_0 => Num0,
            Keysym::_1 => Num1,
            Keysym::_2 => Num2,
            Keysym::_3 => Num3,
            Keysym::_4 => Num4,
            Keysym::_5 => Num5,
            Keysym::_6 => Num6,
            Keysym::_7 => Num7,
            Keysym::_8 => Num8,
            Keysym::_9 => Num9,
            Keysym::a => A,
            Keysym::b => B,
            Keysym::c => C,
            Keysym::d => D,
            Keysym::e => E,
            Keysym::f => F,
            Keysym::g => G,
            Keysym::h => H,
            Keysym::i => I,
            Keysym::j => J,
            Keysym::k => K,
            Keysym::l => L,
            Keysym::m => M,
            Keysym::n => N,
            Keysym::o => O,
            Keysym::p => P,
            Keysym::q => Q,
            Keysym::r => R,
            Keysym::s => S,
            Keysym::t => T,
            Keysym::u => U,
            Keysym::v => V,
            Keysym::w => W,
            Keysym::x => X,
            Keysym::y => Y,
            Keysym::z => Z,
            _ => {
                return Err(());
            }
        })
    }
}

/// Convert from smithay's [`ModifiersState`] to egui's [`Modifiers`]
pub fn convert_modifiers(modifiers: ModifiersState) -> Modifiers {
    ModifiersWrapper(modifiers).into()
}

pub struct ModifiersWrapper(pub ModifiersState);

impl From<ModifiersWrapper> for Modifiers {
    fn from(modifiers: ModifiersWrapper) -> Modifiers {
        Modifiers {
            alt: modifiers.0.alt,
            ctrl: modifiers.0.ctrl,
            shift: modifiers.0.shift,
            mac_cmd: if cfg!(target_os = "macos") {
                modifiers.0.logo
            } else {
                false
            },
            command: if cfg!(target_os = "macos") {
                modifiers.0.logo
            } else {
                modifiers.0.ctrl
            },
        }
    }
}

/// Convert from smithay's [`MouseButton`] to egui's [`PointerButton`], if possible
pub fn convert_button(button: MouseButton) -> Option<PointerButton> {
    ButtonWrapper(button).try_into().ok()
}

pub struct ButtonWrapper(pub MouseButton);

impl TryFrom<ButtonWrapper> for PointerButton {
    type Error = ();

    fn try_from(button: ButtonWrapper) -> Result<PointerButton, ()> {
        Ok(match button.0 {
            MouseButton::Left => PointerButton::Primary,
            MouseButton::Middle => PointerButton::Middle,
            MouseButton::Right => PointerButton::Secondary,
            _ => {
                return Err(());
            }
        })
    }
}
