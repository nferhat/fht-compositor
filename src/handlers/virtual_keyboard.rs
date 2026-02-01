use smithay::wayland::virtual_keyboard::VirtualKeyboardHandler;

use crate::state::State;

// FIXME: IMPL

impl VirtualKeyboardHandler for State {
    fn on_keyboard_event(
        &mut self,
        keycode: smithay::backend::input::Keycode,
        state: smithay::backend::input::KeyState,
        time: u32,
        keyboard: smithay::input::keyboard::KeyboardHandle<Self>,
    ) {
        _ = (keycode, state, time, keyboard);
    }

    fn on_keyboard_modifiers(
        &mut self,
        depressed_mods: smithay::input::keyboard::xkb::ModMask,
        latched_mods: smithay::input::keyboard::xkb::ModMask,
        locked_mods: smithay::input::keyboard::xkb::ModMask,
        keyboard: smithay::input::keyboard::KeyboardHandle<Self>,
    ) {
        _ = (depressed_mods, latched_mods, locked_mods, keyboard);
    }
}

smithay::delegate_virtual_keyboard_manager!(crate::state::State);
