use std::sync::Arc;

use fht_compositor_config::MouseAction;
use smithay::utils::{Rectangle, Serial};

use crate::state::State;
use crate::utils::output::OutputExt;
use crate::utils::RectCenterExt;

#[derive(Debug, Clone)]
pub enum KeyAction {
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
    FocusNextWindow,
    FocusPreviousWindow,
    SwapWithNextWindow,
    SwapWithPreviousWindow,
    FocusNextOutput,
    FocusPreviousOutput,
    CloseFocusedWindow,
    FocusWorkspace(usize),
    SendFocusedWindowToWorkspace(usize),
    None,
}

impl From<fht_compositor_config::KeyActionDesc> for KeyAction {
    fn from(value: fht_compositor_config::KeyActionDesc) -> Self {
        match value {
            fht_compositor_config::KeyActionDesc::Simple(value) => match value {
                fht_compositor_config::SimpleKeyAction::Quit => Self::Quit,
                fht_compositor_config::SimpleKeyAction::ReloadConfig => Self::ReloadConfig,
                fht_compositor_config::SimpleKeyAction::SelectNextLayout => Self::SelectNextLayout,
                fht_compositor_config::SimpleKeyAction::SelectPreviousLayout => {
                    Self::SelectPreviousLayout
                }
                fht_compositor_config::SimpleKeyAction::MaximizeFocusedWindow => {
                    Self::MaximizeFocusedWindow
                }
                fht_compositor_config::SimpleKeyAction::FullscreenFocusedWindow => {
                    Self::FullscreenFocusedWindow
                }
                fht_compositor_config::SimpleKeyAction::FocusNextWindow => Self::FocusNextWindow,
                fht_compositor_config::SimpleKeyAction::FocusPreviousWindow => {
                    Self::FocusPreviousWindow
                }
                fht_compositor_config::SimpleKeyAction::SwapWithNextWindow => {
                    Self::SwapWithNextWindow
                }
                fht_compositor_config::SimpleKeyAction::SwapWithPreviousWindow => {
                    Self::SwapWithPreviousWindow
                }
                fht_compositor_config::SimpleKeyAction::FocusNextOutput => Self::FocusNextOutput,
                fht_compositor_config::SimpleKeyAction::FocusPreviousOutput => {
                    Self::FocusPreviousOutput
                }
                fht_compositor_config::SimpleKeyAction::CloseFocusedWindow => {
                    Self::CloseFocusedWindow
                }
                fht_compositor_config::SimpleKeyAction::None => Self::None,
            },
            fht_compositor_config::KeyActionDesc::Complex { action } => match action {
                fht_compositor_config::ComplexKeyAction::Quit => Self::Quit,
                fht_compositor_config::ComplexKeyAction::ReloadConfig => Self::ReloadConfig,
                fht_compositor_config::ComplexKeyAction::SelectNextLayout => Self::SelectNextLayout,
                fht_compositor_config::ComplexKeyAction::SelectPreviousLayout => {
                    Self::SelectPreviousLayout
                }
                fht_compositor_config::ComplexKeyAction::MaximizeFocusedWindow => {
                    Self::MaximizeFocusedWindow
                }
                fht_compositor_config::ComplexKeyAction::FullscreenFocusedWindow => {
                    Self::FullscreenFocusedWindow
                }
                fht_compositor_config::ComplexKeyAction::FocusNextWindow => Self::FocusNextWindow,
                fht_compositor_config::ComplexKeyAction::FocusPreviousWindow => {
                    Self::FocusPreviousWindow
                }
                fht_compositor_config::ComplexKeyAction::SwapWithNextWindow => {
                    Self::SwapWithNextWindow
                }
                fht_compositor_config::ComplexKeyAction::SwapWithPreviousWindow => {
                    Self::SwapWithPreviousWindow
                }
                fht_compositor_config::ComplexKeyAction::FocusNextOutput => Self::FocusNextOutput,
                fht_compositor_config::ComplexKeyAction::FocusPreviousOutput => {
                    Self::FocusPreviousOutput
                }
                fht_compositor_config::ComplexKeyAction::CloseFocusedWindow => {
                    Self::CloseFocusedWindow
                }
                fht_compositor_config::ComplexKeyAction::None => Self::None,
                fht_compositor_config::ComplexKeyAction::RunCommand(cmd) => Self::RunCommand(cmd),
                fht_compositor_config::ComplexKeyAction::ChangeMwfact(delta) => {
                    Self::ChangeMwfact(delta)
                }
                fht_compositor_config::ComplexKeyAction::ChangeNmaster(delta) => {
                    Self::ChangeNmaster(delta)
                }
                fht_compositor_config::ComplexKeyAction::ChangeWindowProportion(delta) => {
                    Self::ChangeProportion(delta)
                }
                fht_compositor_config::ComplexKeyAction::FocusWorkspace(idx) => {
                    Self::FocusWorkspace(idx)
                }
                fht_compositor_config::ComplexKeyAction::SendToWorkspace(idx) => {
                    Self::SendFocusedWindowToWorkspace(idx)
                }
            },
        }
    }
}

impl State {
    #[profiling::function]
    pub fn process_key_action(&mut self, action: KeyAction) {
        let Some(ref output) = self.fht.focus_state.output.clone() else {
            return;
        };
        let config = Arc::clone(&self.fht.config);
        let active_window = self.fht.space.active_window();
        match action {
            KeyAction::Quit => self.fht.stop = true,
            KeyAction::ReloadConfig => self.reload_config(),
            KeyAction::RunCommand(cmd) => crate::utils::spawn(cmd),
            KeyAction::SelectNextLayout => self.fht.space.select_next_layout(true),
            KeyAction::SelectPreviousLayout => self.fht.space.select_previous_layout(true),
            KeyAction::ChangeMwfact(delta) => self.fht.space.change_mwfact(delta, true),
            KeyAction::ChangeNmaster(delta) => self.fht.space.change_nmaster(delta, true),
            KeyAction::ChangeProportion(delta) => {
                if let Some(window) = active_window {
                    self.fht.space.change_proportion(&window, delta, true)
                }
            }
            KeyAction::MaximizeFocusedWindow => {
                if let Some(window) = active_window {
                    self.fht.space.maximize_window(&window, true, true);
                }
            }
            KeyAction::FullscreenFocusedWindow => {
                if let Some(window) = active_window {
                    if window.fullscreen() {
                        // Workspace will take care of removing fullscreen
                        window.request_fullscreen(false);
                    } else {
                        self.fht.space.fullscreen_window(&window, true);
                    }
                }
            }
            KeyAction::FocusNextWindow => {
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
                    self.set_focus_target(Some(window));
                }
            }
            KeyAction::FocusPreviousWindow => {
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
                    self.set_focus_target(Some(window));
                }
            }
            KeyAction::SwapWithNextWindow => {
                let active = self.fht.space.active_workspace_mut();
                if active.swap_active_tile_with_next(true, true) {
                    let tile = active.active_tile().unwrap();
                    let window = tile.window().clone();
                    if config.general.cursor_warps {
                        let tile_geo = tile.geometry();
                        self.move_pointer(tile_geo.center().to_f64())
                    }
                    self.set_focus_target(Some(window));
                }
            }
            KeyAction::SwapWithPreviousWindow => {
                let active = self.fht.space.active_workspace_mut();
                if active.swap_active_tile_with_previous(true, true) {
                    let tile = active.active_tile().unwrap();
                    let window = tile.window().clone();
                    if config.general.cursor_warps {
                        let tile_geo = tile.geometry();
                        self.move_pointer(tile_geo.center().to_f64())
                    }
                    self.set_focus_target(Some(window));
                }
            }
            KeyAction::FocusNextOutput => {
                let outputs: Vec<_> = self.fht.space.outputs().cloned().collect();
                let outputs_len = outputs.len();
                if outputs_len < 2 {
                    return;
                }

                let current_output_idx = outputs
                    .iter()
                    .position(|o| o == output)
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
                self.fht.focus_state.output.replace(output).unwrap();
            }
            KeyAction::FocusPreviousOutput => {
                let outputs: Vec<_> = self.fht.space.outputs().cloned().collect();
                let outputs_len = outputs.len();
                if outputs_len < 2 {
                    return;
                }

                let current_output_idx = outputs
                    .iter()
                    .position(|o| o == output)
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
                self.fht.focus_state.output.replace(output).unwrap();
            }
            KeyAction::CloseFocusedWindow => {
                if let Some(window) = active_window {
                    window.toplevel().send_close();
                }
            }
            KeyAction::FocusWorkspace(idx) => {
                let mon = self.fht.space.active_monitor_mut();
                if let Some(window) = mon.set_active_workspace_idx(idx) {
                    self.set_focus_target(Some(window));
                }
            }
            KeyAction::SendFocusedWindowToWorkspace(idx) => {
                let active = self.fht.space.active_workspace_mut();
                let Some(window) = active.active_window() else {
                    return;
                };
                if active.remove_window(&window, true) {
                    if let Some(window) = active.active_window() {
                        // Focus the new one now
                        self.set_focus_target(Some(window));
                    }

                    let idx = idx.clamp(0, 9);
                    let mon = self.fht.space.active_monitor_mut();
                    mon.workspace_mut_by_index(idx).insert_window(window, true);
                }
            }
            _ => {}
        }
    }
}

impl State {
    #[profiling::function]
    pub fn process_mouse_action(&mut self, action: MouseAction, _serial: Serial) {
        // TODO: Handle mouse actions again.
        // Currently needs re-implementation from the space side
        match action {
            _ => (),
            // MouseAction::SwapTile => {
            //     if let Some((PointerFocusTarget::Window(window), _)) =
            //         self.fht.focus_target_under(pointer_loc)
            //     {
            //         self.fht.loop_handle.insert_idle(move |state| {
            //             let pointer = state.fht.pointer.clone();
            //             if !pointer.has_grab(serial) {
            //                 return;
            //             }
            //             let Some(start_data) = pointer.grab_start_data() else {
            //                 return;
            //             };
            //             if let Some(workspace) = state.fht.space.workspace_mut_for_window(&window) {
            //                 // TODO: Re-implement swap
            //                 // if workspace.start_interactive_swap(&window) {
            //                 //     state.fht.loop_handle.insert_idle(|state| {
            //                 //         // TODO: Figure out why I have todo this inside a idle
            //                 //         state.fht.interactive_grab_active = true;
            //                 //         state.fht.cursor_theme_manager.set_image_status(
            //                 //             CursorImageStatus::Named(CursorIcon::Grabbing),
            //                 //         );
            //                 //     });
            //                 //     let grab = SwapTileGrab { window, start_data };
            //                 //     pointer.set_grab(state, grab, serial, Focus::Clear);
            //                 // }
            //             }
            //         });
            //     }
            // }
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
