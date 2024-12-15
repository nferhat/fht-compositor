use std::sync::Arc;

use fht_compositor_config::MouseAction;
use smithay::input::pointer::{CursorIcon, CursorImageStatus, Focus};
use smithay::utils::{Point, Rectangle, Serial};

use super::swap_tile_grab::SwapTileGrab;
use crate::focus_target::PointerFocusTarget;
use crate::output::OutputExt;
use crate::state::State;
use crate::utils::RectCenterExt;

/// The "type" of a [`KeyAction`].
///
/// A [`KeyAction`] needs additional data associated with it, for example, whether we should allow
/// it to be executed while the compositor is locked.
#[derive(Debug, Clone)]
pub enum KeyActionType {
    Quit,
    ReloadConfig,
    RunCommand(String),
    SelectNextLayout,
    SelectPreviousLayout,
    ChangeMwfact(f64),
    ChangeNmaster(i32),
    ChangeProportion(f64),
    MaximizeFocusedWindow,
    FullscreenFocusedWindow,
    FloatFocusedWindow,
    MoveFloatingWindow([i32; 2]),
    ResizeFloatingWindow([i32; 2]),
    FocusNextWindow,
    FocusPreviousWindow,
    SwapWithNextWindow,
    SwapWithPreviousWindow,
    FocusNextOutput,
    FocusPreviousOutput,
    CloseFocusedWindow,
    FocusWorkspace(usize),
    SendFocusedWindowToWorkspace(usize),
    FocusNextWorkspace,
    FocusPreviousWorkspace,
    None,
}

/// A [`KeyAction`]. It describes an action to execute when the user hits a specific [`KeyPattern`],
/// I.E a bunch of key actions.
#[derive(Debug, Clone)]
pub struct KeyAction {
    /// The type of the [`KeyAction`], I.E what to do.
    r#type: KeyActionType,
    /// Whether we should allow this [`KeyAction`] to be executed while the compositor is locked.
    allow_while_locked: bool,
}

impl KeyAction {
    /// Create a new dummy [`KeyAction`], with no bound action.
    ///
    /// Binding this is equivalent to disabling the [`KeyPattern`].
    pub const fn none() -> Self {
        Self {
            r#type: KeyActionType::None,
            allow_while_locked: false,
        }
    }
}

impl From<fht_compositor_config::KeyActionDesc> for KeyAction {
    fn from(value: fht_compositor_config::KeyActionDesc) -> Self {
        let r#type;
        let allow_while_locked;

        match value {
            fht_compositor_config::KeyActionDesc::Simple(value) => {
                allow_while_locked = false; // by default, key actions should not run.
                r#type = match value {
                    fht_compositor_config::SimpleKeyAction::Quit => KeyActionType::Quit,
                    fht_compositor_config::SimpleKeyAction::ReloadConfig => {
                        KeyActionType::ReloadConfig
                    }
                    fht_compositor_config::SimpleKeyAction::SelectNextLayout => {
                        KeyActionType::SelectNextLayout
                    }
                    fht_compositor_config::SimpleKeyAction::SelectPreviousLayout => {
                        KeyActionType::SelectPreviousLayout
                    }
                    fht_compositor_config::SimpleKeyAction::MaximizeFocusedWindow => {
                        KeyActionType::MaximizeFocusedWindow
                    }
                    fht_compositor_config::SimpleKeyAction::FullscreenFocusedWindow => {
                        KeyActionType::FullscreenFocusedWindow
                    }
                    fht_compositor_config::SimpleKeyAction::FloatFocusedWindow => {
                        KeyActionType::FloatFocusedWindow
                    }
                    fht_compositor_config::SimpleKeyAction::FocusNextWindow => {
                        KeyActionType::FocusNextWindow
                    }
                    fht_compositor_config::SimpleKeyAction::FocusPreviousWindow => {
                        KeyActionType::FocusPreviousWindow
                    }
                    fht_compositor_config::SimpleKeyAction::SwapWithNextWindow => {
                        KeyActionType::SwapWithNextWindow
                    }
                    fht_compositor_config::SimpleKeyAction::SwapWithPreviousWindow => {
                        KeyActionType::SwapWithPreviousWindow
                    }
                    fht_compositor_config::SimpleKeyAction::FocusNextOutput => {
                        KeyActionType::FocusNextOutput
                    }
                    fht_compositor_config::SimpleKeyAction::FocusPreviousOutput => {
                        KeyActionType::FocusPreviousOutput
                    }
                    fht_compositor_config::SimpleKeyAction::CloseFocusedWindow => {
                        KeyActionType::CloseFocusedWindow
                    }
                    fht_compositor_config::SimpleKeyAction::FocusNextWorkspace => {
                        KeyActionType::FocusNextWorkspace
                    }
                    fht_compositor_config::SimpleKeyAction::FocusPreviousWorkspace => {
                        KeyActionType::FocusPreviousWorkspace
                    }
                    fht_compositor_config::SimpleKeyAction::None => KeyActionType::None,
                };
            }
            fht_compositor_config::KeyActionDesc::Complex {
                action,
                allow_while_locked: allow_while_locked_value,
            } => {
                allow_while_locked = allow_while_locked_value;
                r#type = match action {
                    fht_compositor_config::ComplexKeyAction::Quit => KeyActionType::Quit,
                    fht_compositor_config::ComplexKeyAction::ReloadConfig => {
                        KeyActionType::ReloadConfig
                    }
                    fht_compositor_config::ComplexKeyAction::SelectNextLayout => {
                        KeyActionType::SelectNextLayout
                    }
                    fht_compositor_config::ComplexKeyAction::SelectPreviousLayout => {
                        KeyActionType::SelectPreviousLayout
                    }
                    fht_compositor_config::ComplexKeyAction::MaximizeFocusedWindow => {
                        KeyActionType::MaximizeFocusedWindow
                    }
                    fht_compositor_config::ComplexKeyAction::FullscreenFocusedWindow => {
                        KeyActionType::FullscreenFocusedWindow
                    }
                    fht_compositor_config::ComplexKeyAction::FloatFocusedWindow => {
                        KeyActionType::FloatFocusedWindow
                    }
                    fht_compositor_config::ComplexKeyAction::MoveFloatingWindow(change) => {
                        KeyActionType::MoveFloatingWindow(change)
                    }
                    fht_compositor_config::ComplexKeyAction::ResizeFloatingWindow(change) => {
                        KeyActionType::ResizeFloatingWindow(change)
                    }
                    fht_compositor_config::ComplexKeyAction::FocusNextWindow => {
                        KeyActionType::FocusNextWindow
                    }
                    fht_compositor_config::ComplexKeyAction::FocusPreviousWindow => {
                        KeyActionType::FocusPreviousWindow
                    }
                    fht_compositor_config::ComplexKeyAction::SwapWithNextWindow => {
                        KeyActionType::SwapWithNextWindow
                    }
                    fht_compositor_config::ComplexKeyAction::SwapWithPreviousWindow => {
                        KeyActionType::SwapWithPreviousWindow
                    }
                    fht_compositor_config::ComplexKeyAction::FocusNextOutput => {
                        KeyActionType::FocusNextOutput
                    }
                    fht_compositor_config::ComplexKeyAction::FocusPreviousOutput => {
                        KeyActionType::FocusPreviousOutput
                    }
                    fht_compositor_config::ComplexKeyAction::FocusNextWorkspace => {
                        KeyActionType::FocusNextWorkspace
                    }
                    fht_compositor_config::ComplexKeyAction::FocusPreviousWorkspace => {
                        KeyActionType::FocusPreviousWorkspace
                    }
                    fht_compositor_config::ComplexKeyAction::CloseFocusedWindow => {
                        KeyActionType::CloseFocusedWindow
                    }
                    fht_compositor_config::ComplexKeyAction::None => KeyActionType::None,
                    fht_compositor_config::ComplexKeyAction::RunCommand(cmd) => {
                        KeyActionType::RunCommand(cmd)
                    }
                    fht_compositor_config::ComplexKeyAction::ChangeMwfact(delta) => {
                        KeyActionType::ChangeMwfact(delta)
                    }
                    fht_compositor_config::ComplexKeyAction::ChangeNmaster(delta) => {
                        KeyActionType::ChangeNmaster(delta)
                    }
                    fht_compositor_config::ComplexKeyAction::ChangeWindowProportion(delta) => {
                        KeyActionType::ChangeProportion(delta)
                    }
                    fht_compositor_config::ComplexKeyAction::FocusWorkspace(idx) => {
                        KeyActionType::FocusWorkspace(idx)
                    }
                    fht_compositor_config::ComplexKeyAction::SendToWorkspace(idx) => {
                        KeyActionType::SendFocusedWindowToWorkspace(idx)
                    }
                };
            }
        }

        Self {
            r#type,
            allow_while_locked,
        }
    }
}

impl State {
    pub fn process_key_action(&mut self, action: KeyAction) {
        crate::profile_function!();
        if self.fht.is_locked() && !action.allow_while_locked {
            return;
        }

        let output = self.fht.space.active_output().clone();
        let config = Arc::clone(&self.fht.config);
        let active_window = self.fht.space.active_window();

        match action.r#type {
            KeyActionType::Quit => self.fht.stop = true,
            KeyActionType::ReloadConfig => self.reload_config(),
            KeyActionType::RunCommand(cmd) => crate::utils::spawn(cmd),
            KeyActionType::SelectNextLayout => self.fht.space.select_next_layout(true),
            KeyActionType::SelectPreviousLayout => self.fht.space.select_previous_layout(true),
            KeyActionType::ChangeMwfact(delta) => self.fht.space.change_mwfact(delta, true),
            KeyActionType::ChangeNmaster(delta) => self.fht.space.change_nmaster(delta, true),
            KeyActionType::ChangeProportion(delta) => {
                if let Some(window) = active_window {
                    self.fht.space.change_proportion(&window, delta, true)
                }
            }
            KeyActionType::MaximizeFocusedWindow => {
                if let Some(window) = active_window {
                    let prev = window.maximized();
                    self.fht.space.maximize_window(&window, !prev, true);
                }
            }
            KeyActionType::FullscreenFocusedWindow => {
                if let Some(window) = active_window {
                    if window.fullscreen() {
                        // Workspace will take care of removing fullscreen
                        window.request_fullscreen(false);
                    } else {
                        window.request_fullscreen(true);
                        self.fht.space.fullscreen_window(&window, true);
                    }
                }
            }
            KeyActionType::FloatFocusedWindow => {
                let active = self.fht.space.active_workspace_mut();
                if let Some(tile) = active.active_tile() {
                    let prev = tile.window().tiled();
                    tile.window().request_tiled(!prev);
                }
                active.arrange_tiles(true);
            }
            KeyActionType::MoveFloatingWindow([dx, dy]) => {
                let active = self.fht.space.active_workspace_mut();
                if let Some(tile) = active.active_tile_mut() {
                    if !tile.window().tiled() {
                        let new_loc = tile.location() + Point::from((dx, dy));
                        tile.set_location(new_loc, true);
                    }
                }
            }
            KeyActionType::ResizeFloatingWindow([dx, dy]) => {
                let active = self.fht.space.active_workspace_mut();
                if let Some(tile) = active.active_tile_mut() {
                    if !tile.window().tiled() {
                        let mut new_size = tile.size();
                        // Clamp at 25 minimum to avoid making the tile useless as well as avoiding
                        // to crash smithay code
                        new_size.w = (new_size.w + dx).max(25);
                        new_size.h = (new_size.h + dy).max(25);
                        tile.set_size(new_size, true);
                    }
                }
            }
            KeyActionType::FocusNextWindow => {
                let active = self.fht.space.active_workspace_mut();
                if let Some(window) = active.activate_next_tile(true) {
                    if config.general.cursor_warps {
                        let window_geometry = Rectangle::from_loc_and_size(
                            active.window_location(&window).unwrap()
                                + active.output().current_location(),
                            window.size(),
                        );

                        self.move_pointer(window_geometry.center().to_f64())
                    }
                    self.set_keyboard_focus(Some(window));
                }
            }
            KeyActionType::FocusPreviousWindow => {
                let active = self.fht.space.active_workspace_mut();
                if let Some(window) = active.activate_previous_tile(true) {
                    if config.general.cursor_warps {
                        let window_geometry = Rectangle::from_loc_and_size(
                            active.window_location(&window).unwrap()
                                + active.output().current_location(),
                            window.size(),
                        );

                        self.move_pointer(window_geometry.center().to_f64())
                    }
                    self.set_keyboard_focus(Some(window));
                }
            }
            KeyActionType::SwapWithNextWindow => {
                let active = self.fht.space.active_workspace_mut();
                if active.swap_active_tile_with_next(true, true) {
                    let tile = active.active_tile().unwrap();
                    let window = tile.window().clone();
                    if config.general.cursor_warps {
                        let tile_geo = tile.geometry();
                        self.move_pointer(tile_geo.center().to_f64())
                    }
                    self.set_keyboard_focus(Some(window));
                }
            }
            KeyActionType::SwapWithPreviousWindow => {
                let active = self.fht.space.active_workspace_mut();
                if active.swap_active_tile_with_previous(true, true) {
                    let tile = active.active_tile().unwrap();
                    let window = tile.window().clone();
                    if config.general.cursor_warps {
                        let tile_geo = tile.geometry();
                        self.move_pointer(tile_geo.center().to_f64())
                    }
                    self.set_keyboard_focus(Some(window));
                }
            }
            KeyActionType::FocusNextOutput => {
                let outputs: Vec<_> = self.fht.space.outputs().cloned().collect();
                let outputs_len = outputs.len();
                if outputs_len < 2 {
                    return;
                }

                let current_output_idx = outputs
                    .iter()
                    .position(|o| *o == output)
                    .expect("Focused output is not registered");

                let mut next_output_idx = current_output_idx + 1;
                if next_output_idx == outputs_len {
                    next_output_idx = 0;
                }

                let output = outputs.into_iter().skip(next_output_idx).next().unwrap();
                if config.general.cursor_warps {
                    let center = output.geometry().center();
                    self.move_pointer(center.to_f64());
                }
                self.fht.space.set_active_output(&output);
            }
            KeyActionType::FocusPreviousOutput => {
                let outputs: Vec<_> = self.fht.space.outputs().cloned().collect();
                let outputs_len = outputs.len();
                if outputs_len < 2 {
                    return;
                }

                let current_output_idx = outputs
                    .iter()
                    .position(|o| *o == output)
                    .expect("Focused output is not registered");

                let next_output_idx = match current_output_idx.checked_sub(1) {
                    Some(idx) => idx,
                    None => outputs_len - 1,
                };

                let output = outputs.into_iter().skip(next_output_idx).next().unwrap();
                if config.general.cursor_warps {
                    let center = output.geometry().center();
                    self.move_pointer(center.to_f64());
                }
                self.fht.space.set_active_output(&output);
            }
            KeyActionType::CloseFocusedWindow => {
                if let Some(window) = active_window {
                    window.toplevel().send_close();
                }
            }
            KeyActionType::FocusWorkspace(idx) => {
                let mon = self.fht.space.active_monitor_mut();
                if let Some(window) = mon.set_active_workspace_idx(idx, true) {
                    self.set_keyboard_focus(Some(window));
                }
            }
            KeyActionType::FocusNextWorkspace => {
                let mon = self.fht.space.active_monitor_mut();
                let idx = (mon.active_workspace_idx() + 1).clamp(0, 8);
                if let Some(window) = mon.set_active_workspace_idx(idx, true) {
                    self.set_keyboard_focus(Some(window));
                }
            }
            KeyActionType::FocusPreviousWorkspace => {
                let mon = self.fht.space.active_monitor_mut();
                let idx = mon.active_workspace_idx().saturating_sub(1);
                if let Some(window) = mon.set_active_workspace_idx(idx, true) {
                    self.set_keyboard_focus(Some(window));
                }
            }
            KeyActionType::SendFocusedWindowToWorkspace(idx) => {
                let active = self.fht.space.active_workspace_mut();
                let Some(window) = active.active_window() else {
                    return;
                };
                if active.remove_window(&window, true) {
                    if let Some(window) = active.active_window() {
                        // Focus the new one now
                        self.set_keyboard_focus(Some(window));
                    }

                    let idx = idx.clamp(0, 9);
                    let mon = self.fht.space.active_monitor_mut();
                    mon.workspace_mut_by_index(idx).insert_window(window, true);
                }
            }
            KeyActionType::None => (), // disabled the key combo
        }
    }
}

impl State {
    pub fn process_mouse_action(&mut self, action: MouseAction, serial: Serial) {
        crate::profile_function!();
        match action {
            MouseAction::SwapTile => {
                let pointer_loc = self.fht.pointer.current_location();
                if let Some((PointerFocusTarget::Window(window), _)) =
                    self.fht.focus_target_under(pointer_loc)
                {
                    self.fht.loop_handle.insert_idle(move |state| {
                        let pointer = state.fht.pointer.clone();
                        if !pointer.has_grab(serial) {
                            return;
                        }
                        let Some(start_data) = pointer.grab_start_data() else {
                            return;
                        };

                        if state.fht.space.start_interactive_swap(&window) {
                            state.fht.loop_handle.insert_idle(|state| {
                                // TODO: Figure out why I have todo this inside a idle
                                state.fht.interactive_grab_active = true;
                                state.fht.cursor_theme_manager.set_image_status(
                                    CursorImageStatus::Named(CursorIcon::Grabbing),
                                );
                            });
                            let grab = SwapTileGrab { window, start_data };
                            pointer.set_grab(state, grab, serial, Focus::Clear);
                        }
                    });
                }
            }
            _ => (),
            // MouseAction::ResizeTile => {
            //     if let Some((PointerFocusTarget::Window(window), _)) =
            //         self.fht.focus_target_under(pointer_loc)
            //     {
            //         let pointer_loc = self.fht.pointer.current_location();
            //         let Rectangle { loc, size } =
            //             self.fht.window_visual_geometry(&window).unwrap().to_f64();
            //
            //         let pointer_loc_in_window = pointer_loc - loc;
            //         if window.surface_under(pointer_loc_in_window,
            // WindowSurfaceType::ALL).is_none() {             return;
            //         }
            //
            //         // We divide the window into 9 sections, so that if you grab for example
            //         // somewhere in the middle of the bottom edge, you can only resize
            // vertically.         let mut edges = ResizeEdge::empty();
            //         if pointer_loc_in_window.x < size.w / 3. {
            //             edges |= ResizeEdge::LEFT;
            //         } else if 2. * size.w / 3. < pointer_loc_in_window.x {
            //             edges |= ResizeEdge::RIGHT;
            //         }
            //         if pointer_loc_in_window.y < size.h / 3. {
            //             edges |= ResizeEdge::TOP;
            //         } else if 2. * size.h / 3. < pointer_loc_in_window.y {
            //             edges |= ResizeEdge::BOTTOM;
            //         }
            //
            //         self.fht.loop_handle.insert_idle(move |state| {
            //             state.handle_resize_request(window, serial, edges)
            //         });
            //     }
            // }
        }
    }
}
