//! `hyprland-global-shortcuts-v1` protocol support.
//!
//! This protocol is from Hyprland and is mostly implemented to help Sleex transition into being
//! used in `fht-compositor`. Shortcuts can only be triggered if they are explicitly configured.

use std::collections::HashMap;
use std::time::Duration;

use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, Resource,
};

use crate::protocols::raw::hyprland_global_shortcuts_v1::hyprland_global_shortcut_v1::HyprlandGlobalShortcutV1;
use crate::protocols::raw::hyprland_global_shortcuts_v1::hyprland_global_shortcuts_manager_v1::{
    self, HyprlandGlobalShortcutsManagerV1,
};
use crate::state::State;
use crate::utils::split_timestamp;

const VERSION: u32 = 1;

pub struct HyprlandGlobalShortcutsManagerGlobalData {
    filter: Box<dyn Fn(&Client) -> bool + Send + Sync>,
}

/// Metadata attached to every `hyprland_global_shortcut_v1` resource.
#[derive(Debug, Clone)]
pub struct ShortcutData {
    pub app_id: String,
    pub id: String,
    pub description: String,
    pub trigger_description: String,
}

pub struct HyprlandGlobalShortcutsState {
    /// Live shortcut resources keyed by `(app_id, id)`.
    shortcuts: HashMap<(String, String), HyprlandGlobalShortcutV1>,
}

impl std::fmt::Debug for HyprlandGlobalShortcutsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HyprlandGlobalShortcutsState")
            .field("shortcuts_count", &self.shortcuts.len())
            .finish()
    }
}

impl HyprlandGlobalShortcutsState {
    /// Create the global and return a new state object.
    ///
    /// `filter` controls which clients may bind the global (e.g. restrict to trusted clients).
    pub fn new<F>(display: &DisplayHandle, filter: F) -> Self
    where
        F: Fn(&Client) -> bool + Send + Sync + 'static,
    {
        let global_data = HyprlandGlobalShortcutsManagerGlobalData {
            filter: Box::new(filter),
        };
        display.create_global::<State, HyprlandGlobalShortcutsManagerV1, _>(VERSION, global_data);
        Self {
            shortcuts: HashMap::new(),
        }
    }

    /// Look up a registered shortcut by its `(app_id, id)` key.
    fn get(&self, app_id: &str, id: &str) -> Option<&HyprlandGlobalShortcutV1> {
        self.shortcuts.get(&(app_id.to_owned(), id.to_owned()))
    }

    /// Returns `true` if a shortcut identified by `(app_id, id)` is currently registered and
    /// alive.
    pub fn has_shortcut(&self, app_id: &str, id: &str) -> bool {
        self.get(app_id, id).map_or(false, |s| s.is_alive())
    }

    /// Returns the metadata of all currently registered and alive shortcuts.
    pub fn list_shortcuts(&self) -> Vec<&ShortcutData> {
        self.shortcuts
            .values()
            .filter(|s| s.is_alive())
            .filter_map(|s| s.data::<ShortcutData>())
            .collect()
    }

    /// Fire a `pressed` event on the shortcut identified by `(app_id, id)`.
    ///
    /// Returns `true` if the shortcut existed and the event was sent, `false` otherwise.
    pub fn press_shortcut(&self, app_id: &str, id: &str, time: Duration) -> bool {
        if let Some(shortcut) = self.get(app_id, id) {
            if shortcut.is_alive() {
                let (tv_sec_hi, tv_sec_lo, tv_nsec) = split_timestamp(time);
                shortcut.pressed(tv_sec_hi, tv_sec_lo, tv_nsec);
                return true;
            }
        }
        false
    }

    /// Fire a `released` event on the shortcut identified by `(app_id, id)`.
    ///
    /// Returns `true` if the shortcut existed and the event was sent, `false` otherwise.
    pub fn release_shortcut(&self, app_id: &str, id: &str, time: Duration) -> bool {
        if let Some(shortcut) = self.get(app_id, id) {
            if shortcut.is_alive() {
                let (tv_sec_hi, tv_sec_lo, tv_nsec) = split_timestamp(time);
                shortcut.released(tv_sec_hi, tv_sec_lo, tv_nsec);
                return true;
            }
        }
        false
    }
}

pub trait HyprlandGlobalShortcutsHandler {
    fn hyprland_global_shortcuts_state(&mut self) -> &mut HyprlandGlobalShortcutsState;
    fn new_shortcut(&mut self, data: ShortcutData);
    fn shortcut_destroyed(&mut self, app_id: String, id: String);
}

impl
    GlobalDispatch<
        HyprlandGlobalShortcutsManagerV1,
        HyprlandGlobalShortcutsManagerGlobalData,
        State,
    > for HyprlandGlobalShortcutsState
{
    fn bind(
        _state: &mut State,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: smithay::reexports::wayland_server::New<HyprlandGlobalShortcutsManagerV1>,
        _global_data: &HyprlandGlobalShortcutsManagerGlobalData,
        data_init: &mut DataInit<'_, State>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global_data: &HyprlandGlobalShortcutsManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

// ---------------------------------------------------------------------------
// Dispatch — manager requests
// ---------------------------------------------------------------------------

impl Dispatch<HyprlandGlobalShortcutsManagerV1, (), State> for HyprlandGlobalShortcutsState {
    fn request(
        state: &mut State,
        _client: &Client,
        manager: &HyprlandGlobalShortcutsManagerV1,
        request: hyprland_global_shortcuts_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            hyprland_global_shortcuts_manager_v1::Request::RegisterShortcut {
                shortcut,
                id,
                app_id,
                description,
                trigger_description,
            } => {
                let key = (app_id.clone(), id.clone());

                // Check for duplicates — the protocol requires an already_taken error.
                let handler_state = state.hyprland_global_shortcuts_state();
                if handler_state.shortcuts.contains_key(&key) {
                    manager.post_error(
                        hyprland_global_shortcuts_manager_v1::Error::AlreadyTaken,
                        format!("This '{app_id}:{id}' shortcut has already been registered"),
                    );
                    return;
                }

                let shortcut_data = ShortcutData {
                    app_id: app_id.clone(),
                    id: id.clone(),
                    description: description.clone(),
                    trigger_description: trigger_description.clone(),
                };

                let resource = data_init.init(shortcut, shortcut_data.clone());
                state
                    .hyprland_global_shortcuts_state()
                    .shortcuts
                    .insert(key, resource);

                state.new_shortcut(shortcut_data);
            }
            hyprland_global_shortcuts_manager_v1::Request::Destroy => {
                // Manager object destroyed — individual shortcuts remain valid until they are
                // explicitly destroyed, as per the protocol spec.
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        }
    }
}

impl Dispatch<HyprlandGlobalShortcutV1, ShortcutData, State> for HyprlandGlobalShortcutsState {
    fn request(
        state: &mut State,
        _client: &Client,
        _resource: &HyprlandGlobalShortcutV1,
        request: <HyprlandGlobalShortcutV1 as Resource>::Request,
        data: &ShortcutData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        use crate::protocols::raw::hyprland_global_shortcuts_v1::hyprland_global_shortcut_v1::Request;
        match request {
            Request::Destroy => {
                let key = (data.app_id.clone(), data.id.clone());
                state
                    .hyprland_global_shortcuts_state()
                    .shortcuts
                    .remove(&key);
                state.shortcut_destroyed(data.app_id.clone(), data.id.clone());
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        }
    }
}

#[macro_export]
macro_rules! delegate_hyprland_global_shortcuts {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty:ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!(
            $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
                $crate::protocols::raw::hyprland_global_shortcuts_v1::hyprland_global_shortcuts_manager_v1::HyprlandGlobalShortcutsManagerV1:
                    $crate::protocols::hyprland_global_shortcuts::HyprlandGlobalShortcutsManagerGlobalData
            ] => $crate::protocols::hyprland_global_shortcuts::HyprlandGlobalShortcutsState
        );

        smithay::reexports::wayland_server::delegate_dispatch!(
            $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
                $crate::protocols::raw::hyprland_global_shortcuts_v1::hyprland_global_shortcuts_manager_v1::HyprlandGlobalShortcutsManagerV1: ()
            ] => $crate::protocols::hyprland_global_shortcuts::HyprlandGlobalShortcutsState
        );

        smithay::reexports::wayland_server::delegate_dispatch!(
            $(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
                $crate::protocols::raw::hyprland_global_shortcuts_v1::hyprland_global_shortcut_v1::HyprlandGlobalShortcutV1:
                    $crate::protocols::hyprland_global_shortcuts::ShortcutData
            ] => $crate::protocols::hyprland_global_shortcuts::HyprlandGlobalShortcutsState
        );
    };
}
