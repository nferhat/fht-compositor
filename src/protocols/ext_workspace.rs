//! ext-workspace-v1 support.
//!
//! The protocol treats workspace/virtual desktops as groups of workspace. In `fht-compositor`, we
//! can nicely map this scheme to [`Monitor`]s, where the workspaces are arranged on a
//! (theoretically) finite horizontal strip.
//!
//! Credits to cosmic-comp and YaLTeR/niri for the implementation!

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use smithay::output::Output;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::{DataInit, New, Resource};
use smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1};
use smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1};
use smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1};
use smithay::reexports::wayland_server::{Client, Dispatch, DisplayHandle, GlobalDispatch};

use crate::space::WorkspaceId;
use crate::state::State;

const VERSION: u32 = 1;

pub struct ExtWorkspaceGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
}

pub trait ExtWorkspaceHandler {
    fn ext_workspace_manager_state(&mut self) -> &mut ExtWorkspaceManagerState;
    fn activate_workspace(&mut self, id: WorkspaceId);
}

pub struct ExtWorkspaceManagerState {
    display: DisplayHandle,
    instances: HashMap<ExtWorkspaceManagerV1, Vec<Action>>,
    workspace_groups: HashMap<Output, ExtWorkspaceGroupData>,
    workspaces: HashMap<WorkspaceId, ExtWorkspaceData>,
}

impl ExtWorkspaceManagerState {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<ExtWorkspaceManagerV1, ExtWorkspaceGlobalData>,
        D: Dispatch<ExtWorkspaceManagerV1, ()>,
        D: 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let global_data = ExtWorkspaceGlobalData {
            filter: Box::new(filter),
        };
        display.create_global::<D, ExtWorkspaceManagerV1, _>(VERSION, global_data);
        Self {
            display: display.clone(),
            instances: HashMap::new(),
            workspace_groups: HashMap::new(),
            workspaces: HashMap::new(),
        }
    }
}

struct ExtWorkspaceGroupData {
    instances: Vec<ExtWorkspaceGroupHandleV1>,
}

impl ExtWorkspaceGroupData {
    fn add_instance<D>(
        &mut self,
        handle: &DisplayHandle,
        client: &Client,
        manager: &ExtWorkspaceManagerV1,
        output: &Output,
    ) -> &ExtWorkspaceGroupHandleV1
    where
        D: Dispatch<ExtWorkspaceGroupHandleV1, ExtWorkspaceManagerV1>,
        D: 'static,
    {
        let group = client
            .create_resource::<ExtWorkspaceGroupHandleV1, _, D>(
                handle,
                manager.version(),
                manager.clone(),
            )
            .unwrap();
        manager.workspace_group(&group);

        // For now, workspace number is static, IE. you can't assign more or less than nine
        // workspace per group. This might change soon? Depends on what users want.
        group.capabilities(ext_workspace_group_handle_v1::GroupCapabilities::empty());

        for wl_output in output.client_outputs(client) {
            group.output_enter(&wl_output);
        }

        self.instances.push(group);
        self.instances.last().unwrap()
    }
}

enum Action {
    Activate { id: WorkspaceId },
}

struct ExtWorkspaceData {
    // The name is assured to be unique to the workspace and persistent across output, todo this,
    // we don't use the WorkspaceId used to track stuff with the state and IPC, and instead
    // give a human-readable name in the form of <output-name>-<ws-idx>
    name: Arc<str>,
    // The actual ID  of the workspace in the compositor, used for activating.
    id: WorkspaceId,
    coordinates: [u32; 2],
    state: ext_workspace_handle_v1::State,
    instances: Vec<ExtWorkspaceHandleV1>,
    // A workspace is assured to have an output.
    output: Output,
}

impl ExtWorkspaceData {
    fn add_instance<D>(
        &mut self,
        handle: &DisplayHandle,
        client: &Client,
        manager: &ExtWorkspaceManagerV1,
    ) -> &ExtWorkspaceHandleV1
    where
        D: Dispatch<ExtWorkspaceHandleV1, ExtWorkspaceManagerV1>,
        D: 'static,
    {
        let workspace = client
            .create_resource::<ExtWorkspaceHandleV1, _, D>(
                handle,
                manager.version(),
                manager.clone(),
            )
            .unwrap();
        manager.workspace(&workspace);

        // NOTE: For now we don't expose workspace IDs since they are not stable at all, due to the
        // fact they are unstable across restarts. We depend on connector detection, but
        // it's not reliable.

        workspace.id(format!("{}", *self.id));
        workspace.name(self.name.to_string());
        workspace.coordinates(
            self.coordinates
                .iter()
                .flat_map(|x| x.to_ne_bytes())
                .collect(),
        );
        workspace.state(self.state);
        // Workspaces are static, IE when they are placed inside an output, they are never changing
        // places. This might change if enough people ask for it.
        workspace.capabilities(ext_workspace_handle_v1::WorkspaceCapabilities::Activate);

        self.instances.push(workspace);
        self.instances.last().unwrap()
    }
}

/// Refresh the ext_workspace state.
pub fn refresh(state: &mut State) {
    crate::profile_function!();

    let manager_state = &mut state.fht.ext_workspace_manager_state;
    let mut changed = false;

    // Remove workspace groups of removed outputs.
    let outputs: HashSet<_> = state.fht.space.outputs().collect();
    manager_state.workspace_groups.retain(|output, data| {
        if outputs.contains(output) {
            return true;
        }

        for group in &data.instances {
            let manager: &ExtWorkspaceManagerV1 = group.data().unwrap();
            for ws_data in manager_state.workspaces.values() {
                // Remove all instances of the workspace in all managers.
                for workspace in &ws_data.instances {
                    if workspace.data() == Some(manager) {
                        group.workspace_leave(workspace);
                    }
                }
            }

            group.removed();
        }

        changed = true;
        false
    });

    // Now refresh each workspace groups and its respective workspaces.
    for monitor in state.fht.space.monitors() {
        for (index, workspace) in monitor.workspaces().enumerate() {
            changed |= refresh_workspace(manager_state, monitor, workspace, index);
        }

        // We must refresh the workspace group after declaring the group workspaces above,
        // otheriwse they won't get assigned immediatly.
        changed |= refresh_workspace_group(manager_state, monitor.output());
    }

    // At the end notify changes.
    if changed {
        notify_changes(manager_state);
    }
}

fn refresh_workspace(
    manager_state: &mut ExtWorkspaceManagerState,
    monitor: &crate::space::Monitor,
    workspace: &crate::space::Workspace,
    index: usize,
) -> bool {
    let mut state = ext_workspace_handle_v1::State::empty();
    if monitor.active_workspace_idx() == index {
        state |= ext_workspace_handle_v1::State::Active;
        // NOTE: Hidden in this case means ignored, not not displayed, so we don't bother
        // setting that at all.
    }

    match manager_state.workspaces.entry(workspace.id()) {
        Entry::Vacant(vacant_entry) => {
            // Workspace didn't exist before, initialize it.
            // This should be hit when an output is newly connected.

            let mut data = ExtWorkspaceData {
                name: format!("{}-{}", monitor.output().name(), index).into(),
                id: workspace.id(),
                coordinates: [index as u32; 2],
                state,
                instances: Vec::new(),
                output: monitor.output().clone(),
            };

            // Notify it to all managers
            for manager in manager_state.instances.keys() {
                if let Some(client) = manager.client() {
                    data.add_instance::<State>(&manager_state.display, &client, manager);
                }
            }

            send_workspace_enter_leave(&manager_state.workspace_groups, &data, true);
            vacant_entry.insert(data);

            true
        }
        Entry::Occupied(occupied_entry) => {
            // Existing workspace, check if anything changed.
            // For us, its quite easy (at least with the current workspace system implementation):
            //
            // 1. Workspace output are assured to NEVER change
            // 2. Workspace indexes are assured to NEVER change => coordinates never change
            //
            // So in reality, we only care about updating the active state.
            let data = occupied_entry.into_mut();

            let mut state_changed = false;
            if data.state != state {
                data.state = state;
                state_changed = true;
            }

            if state_changed {
                for instance in &data.instances {
                    instance.state(data.state);
                }
            }

            state_changed
        }
    }
}

fn send_workspace_enter_leave(
    workspace_groups: &HashMap<Output, ExtWorkspaceGroupData>,
    data: &ExtWorkspaceData,
    enter: bool,
) {
    if let Some(group_data) = workspace_groups.get(&data.output) {
        for group in &group_data.instances {
            let manager: &ExtWorkspaceManagerV1 = group.data().unwrap();
            for workspace in &data.instances {
                if workspace.data() == Some(manager) {
                    if enter {
                        group.workspace_enter(workspace);
                    } else {
                        group.workspace_leave(workspace);
                    }
                }
            }
        }
    }
}

/// Notify about changes. This will send the done event to all manager instances.
///
/// You should not call this unless changes occured, else a protocol error will be raised.
fn notify_changes(manager: &ExtWorkspaceManagerState) {
    for manager in manager.instances.keys() {
        manager.done();
    }
}

fn refresh_workspace_group(manager_state: &mut ExtWorkspaceManagerState, output: &Output) -> bool {
    if manager_state.workspace_groups.contains_key(output) {
        // Workspace groups remain the same no matter what, since they are bound to outputs.
        // The only thing that could change is the initial binding, which is done below.
        return false;
    }

    // New workspace group, start tracking it.
    let mut data = ExtWorkspaceGroupData {
        instances: Vec::new(),
    };

    // Create workspace group handle for each manager instance.
    for manager in manager_state.instances.keys() {
        if let Some(client) = manager.client() {
            data.add_instance::<State>(&manager_state.display, &client, manager, output);
        }
    }

    // Send workspace_enter for all existing workspaces on this output.
    for group in &data.instances {
        let manager: &ExtWorkspaceManagerV1 = group.data().unwrap();
        for (_, ws) in manager_state.workspaces.iter() {
            if ws.output == *output {
                eprintln!("Adding workspace: {:?}", ws.id);
                for workspace in &ws.instances {
                    if workspace.data() == Some(manager) {
                        group.workspace_enter(workspace);
                    }
                }
            }
        }
    }

    manager_state.workspace_groups.insert(output.clone(), data);
    true
}

pub fn on_output_bound(state: &mut State, output: &Output, wl_output: &WlOutput) {
    let Some(client) = wl_output.client() else {
        return;
    };

    let mut sent = false;

    let protocol_state = &mut state.fht.ext_workspace_manager_state;
    if let Some(data) = protocol_state.workspace_groups.get_mut(output) {
        for group in &mut data.instances {
            if group.client().as_ref() != Some(&client) {
                continue;
            }

            group.output_enter(wl_output);
            sent = true;
        }
    }

    if !sent {
        return;
    }

    for manager in protocol_state.instances.keys() {
        if manager.client().as_ref() == Some(&client) {
            manager.done();
        }
    }
}

impl<D> GlobalDispatch<ExtWorkspaceManagerV1, ExtWorkspaceGlobalData, D>
    for ExtWorkspaceManagerState
where
    D: GlobalDispatch<ExtWorkspaceManagerV1, ExtWorkspaceGlobalData>,
    D: Dispatch<ExtWorkspaceManagerV1, ()>,
    D: Dispatch<ExtWorkspaceHandleV1, ExtWorkspaceManagerV1>,
    D: ExtWorkspaceHandler,
{
    fn bind(
        state: &mut D,
        handle: &DisplayHandle,
        client: &Client,
        resource: New<ExtWorkspaceManagerV1>,
        _global_data: &ExtWorkspaceGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let manager = data_init.init(resource, ());
        let state = state.ext_workspace_manager_state();

        // Send existing workspaces to the new client.
        let mut new_workspaces: HashMap<_, Vec<_>> = HashMap::new();
        for data in state.workspaces.values_mut() {
            let output = data.output.clone();
            let workspace = data.add_instance::<State>(handle, client, &manager);
            new_workspaces.entry(output).or_default().push(workspace);
        }

        // Create workspace groups for all outputs.
        for (output, group_data) in &mut state.workspace_groups {
            let group = group_data.add_instance::<State>(handle, client, &manager, output);

            for workspace in new_workspaces.get(output).into_iter().flatten() {
                group.workspace_enter(workspace);
            }
        }

        manager.done();
        state.instances.insert(manager, Vec::new());
    }

    fn can_view(client: Client, global_data: &ExtWorkspaceGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<ExtWorkspaceHandleV1, ExtWorkspaceManagerV1, D> for ExtWorkspaceManagerState
where
    D: Dispatch<ExtWorkspaceHandleV1, ExtWorkspaceManagerV1>,
    D: ExtWorkspaceHandler,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &ExtWorkspaceHandleV1,
        request: ext_workspace_handle_v1::Request,
        data: &ExtWorkspaceManagerV1,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        let protocol_state = state.ext_workspace_manager_state();

        let Some((workspace, _)) = protocol_state
            .workspaces
            .iter()
            .find(|(_, data)| data.instances.contains(resource))
        else {
            return;
        };
        let workspace = *workspace;

        match request {
            ext_workspace_handle_v1::Request::Activate => {
                // FIXME: Workspace activation
                let actions = protocol_state.instances.get_mut(data).unwrap();
                actions.push(Action::Activate { id: workspace });
            }
            _ => (),
        }
    }

    fn destroyed(
        state: &mut D,
        _client: ClientId,
        resource: &ExtWorkspaceHandleV1,
        _data: &ExtWorkspaceManagerV1,
    ) {
        let state = state.ext_workspace_manager_state();
        for data in state.workspaces.values_mut() {
            data.instances.retain(|instance| instance != resource);
        }
    }
}

impl<D> Dispatch<ExtWorkspaceManagerV1, (), D> for ExtWorkspaceManagerState
where
    D: Dispatch<ExtWorkspaceManagerV1, ()>,
    D: ExtWorkspaceHandler,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &ExtWorkspaceManagerV1,
        request: ext_workspace_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ext_workspace_manager_v1::Request::Commit => {
                let protocol_state = state.ext_workspace_manager_state();
                let actions = protocol_state.instances.get_mut(resource).unwrap();
                for action in std::mem::take(actions) {
                    match action {
                        Action::Activate { id } => state.activate_workspace(id),
                    }
                }
            }
            ext_workspace_manager_v1::Request::Stop => {
                resource.finished();

                let state = state.ext_workspace_manager_state();
                state.instances.retain(|x, _| x != resource);

                for data in state.workspace_groups.values_mut() {
                    data.instances
                        .retain(|instance| instance.data() != Some(resource));
                }

                for data in state.workspaces.values_mut() {
                    data.instances
                        .retain(|instance| instance.data() != Some(resource));
                }
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(state: &mut D, _client: ClientId, resource: &ExtWorkspaceManagerV1, _data: &()) {
        let state = state.ext_workspace_manager_state();
        state.instances.retain(|x, _| x != resource);
    }
}

impl<D> Dispatch<ExtWorkspaceGroupHandleV1, ExtWorkspaceManagerV1, D> for ExtWorkspaceManagerState
where
    D: Dispatch<ExtWorkspaceGroupHandleV1, ExtWorkspaceManagerV1>,
    D: ExtWorkspaceHandler,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &ExtWorkspaceGroupHandleV1,
        request: <ExtWorkspaceGroupHandleV1 as Resource>::Request,
        _data: &ExtWorkspaceManagerV1,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            ext_workspace_group_handle_v1::Request::CreateWorkspace { .. } => (),
            ext_workspace_group_handle_v1::Request::Destroy => (),
            _ => unreachable!(),
        }
    }

    fn destroyed(
        state: &mut D,
        _client: ClientId,
        resource: &ExtWorkspaceGroupHandleV1,
        _data: &ExtWorkspaceManagerV1,
    ) {
        let state = state.ext_workspace_manager_state();
        for data in state.workspace_groups.values_mut() {
            data.instances.retain(|instance| instance != resource);
        }
    }
}

#[macro_export]
macro_rules! delegate_ext_workspace {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_manager_v1::ExtWorkspaceManagerV1: $crate::protocols::ext_workspace::ExtWorkspaceGlobalData
        ] => $crate::protocols::ext_workspace::ExtWorkspaceManagerState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_manager_v1::ExtWorkspaceManagerV1: ()
        ] => $crate::protocols::ext_workspace::ExtWorkspaceManagerState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_handle_v1::ExtWorkspaceHandleV1: smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_manager_v1::ExtWorkspaceManagerV1
        ] => $crate::protocols::ext_workspace::ExtWorkspaceManagerState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1: smithay::reexports::wayland_protocols::ext::workspace::v1::server::ext_workspace_manager_v1::ExtWorkspaceManagerV1
        ] => $crate::protocols::ext_workspace::ExtWorkspaceManagerState);
    };
}
