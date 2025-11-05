//! Subscribing functionality for `fht-compositor`
//!
//! Most of the code has been written by @Byson94! Thank you very much

use std::collections::{HashMap, HashSet};
use std::io;
use std::os::unix::net::UnixStream;

use async_channel::Receiver;
use calloop::io::Async;
use fht_compositor_ipc::Event;
use futures_util::io::WriteHalf;
use futures_util::AsyncWriteExt;

use crate::space::{Tile, Workspace};
use crate::state::Fht;
use crate::window::Window;

/// Compositor state to track between the IPC server and each client.
#[derive(Default, Clone, Debug)]
pub struct CompositorState {
    pub windows: HashMap<usize, fht_compositor_ipc::Window>,
    pub focused_window_id: Option<usize>,

    pub workspaces: HashMap<usize, fht_compositor_ipc::Workspace>,
    pub active_workspace_id: usize,

    pub space: fht_compositor_ipc::Space,
    pub layer_shells: Vec<fht_compositor_ipc::LayerShell>,
}

/// Start a subscription for a given [`UnixStream`].
///
/// While the channel is active, or the stream didn't disconnect yet, we keep receiving events and
/// writing them to the subscribed client.
///
/// It is up to the sender to ensure initial state is sent to the subscribed client.
pub(super) async fn start_subscribing(
    rx: Receiver<Event>,
    mut writer: WriteHalf<Async<'static, UnixStream>>,
) -> anyhow::Result<()> {
    while let Ok(event) = rx.recv().await {
        if event == Event::Disconnect {
            break;
        }

        let mut json_string = serde_json::to_string(&event).unwrap();
        json_string.push('\n');
        match writer.write_all(json_string.as_bytes()).await {
            Ok(()) => (),
            // Client disconnected, stop this thread.
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => break,
            Err(err) => anyhow::bail!("Failed to communicate initial state to client: {err:?}"),
        }
    }

    Ok(())
}

impl Fht {
    pub fn refresh_ipc_space(&mut self) {
        let Some(server) = &mut self.ipc_server else {
            return;
        };

        // Accumulated events during this cycle
        let mut events = vec![];
        let mut compositor_state = server.compositor_state.borrow_mut();
        let mut removed_workspaces = Vec::<usize>::new();

        let space = &self.space;
        let mut changed = compositor_state.space.primary_idx != space.primary_monitor_idx()
            || compositor_state.space.active_idx != space.primary_monitor_idx();

        // We have to check for length change for handling intial state (which sets monitors to
        // an empty map {}), Otherwise we are never entering the loop.
        changed |= compositor_state.space.monitors.len() != space.monitors().count();

        // For monitors, we are assured the output name doesn't change aswell as workspace IDs
        // (we don't support moving workspaces)
        if !changed {
            for (name, ipc_mon) in &compositor_state.space.monitors {
                let Some(mon) = space.monitors().find(|mon| mon.output().name() == *name) else {
                    // We can't find the monitor, just override everything since it's missing
                    // (in this case we have disconnected output)
                    removed_workspaces.extend(&ipc_mon.workspaces);
                    changed = true;
                    break;
                };

                changed |= ipc_mon.active_workspace_idx != mon.active_idx;
                changed |= ipc_mon.active != mon.active();

                if changed {
                    break;
                }
            }
        }

        // Broadcast removed workspaces from disconnected monitors.
        events.extend(
            removed_workspaces
                .into_iter()
                .map(|id| Event::WorkspaceRemoved { id }),
        );

        if changed {
            // resend the new space.
            let monitors = space
                .monitors()
                .map(|mon| {
                    let workspaces: [usize; 9] = mon
                        .workspaces()
                        .map(|ws| *ws.id())
                        .collect::<Vec<_>>()
                        .try_into()
                        .expect("always 9 workspaces per monitor");

                    (
                        mon.output().name(),
                        fht_compositor_ipc::Monitor {
                            output: mon.output().name(),
                            workspaces,
                            active: mon.active(),
                            active_workspace_idx: mon.active_workspace_idx(),
                        },
                    )
                })
                .collect();

            let space = fht_compositor_ipc::Space {
                monitors,
                active_idx: space.primary_monitor_idx(),
                primary_idx: space.active_monitor_idx(),
            };
            compositor_state.space = space.clone();
            events.push(Event::Space(space));
        }

        drop(compositor_state); // rust argh
        server.push_events(events);
    }

    /// Refresh the IPC window state. Note that this operation is quite expensive as it will
    /// iterate through all the opened/mapped windows in the [`Space`](Fht::space)
    pub fn refresh_ipc_windows(&mut self) {
        let Some(server) = &mut self.ipc_server else {
            return;
        };

        // Accumulated events during this cycle
        let mut events = vec![];
        let mut compositor_state = server.compositor_state.borrow_mut();
        let ipc_windows = &mut compositor_state.windows;
        let mut seen = HashSet::new();
        let mut focused_window_id = None;

        for monitor in self.space.monitors() {
            for workspace in monitor.workspaces() {
                let workspace_id = workspace.id();
                let active_tile_idx = workspace.active_tile_idx();
                let workspace_active = monitor.active() && workspace.index() == monitor.active_idx;

                let make_ipc_window = |idx, tile: &Tile| {
                    let window = tile.window();
                    let location = tile.location() + tile.window_loc();
                    let size = window.size();

                    fht_compositor_ipc::Window {
                        id: *window.id(),
                        title: window.title(),
                        app_id: window.app_id(),
                        workspace_id: *workspace_id,
                        size: (size.w as u32, size.h as u32),
                        location: location.into(),
                        fullscreened: window.fullscreen(),
                        maximized: window.maximized(),
                        tiled: window.tiled(),
                        activated: Some(idx) == active_tile_idx,
                        focused: workspace_active && Some(idx) == active_tile_idx,
                    }
                };

                for (idx, tile) in workspace.tiles().enumerate() {
                    seen.insert(*tile.window().id());
                    let entry = ipc_windows.entry(*tile.window().id());

                    let window = entry
                        .and_modify(|window| {
                            // If there are any changes, reset.
                            if window_changed(window, *workspace_id, tile) {
                                *window = make_ipc_window(idx, tile);
                                events.push(Event::WindowChanged(window.clone()));
                            }
                        })
                        .or_insert_with(|| {
                            let window = make_ipc_window(idx, tile);
                            events.push(Event::WindowChanged(window.clone()));
                            window
                        });

                    if Some(idx) == active_tile_idx {
                        focused_window_id = Some(window.id);
                    }
                }
            }
        }

        // Only retain the windows we saw. As said above, we already do the check in
        // CompositorState, but still good todo this just in case
        ipc_windows.retain(|&id, _| {
            if !seen.contains(&id) {
                events.push(Event::WindowClosed { id });
                false
            } else {
                true
            }
        });

        if compositor_state.focused_window_id != focused_window_id {
            compositor_state.focused_window_id = focused_window_id;
            events.push(Event::FocusedWindowChanged {
                id: focused_window_id,
            });
        }

        drop(compositor_state); // rust argh!
        server.push_events(events);
    }

    pub fn refresh_ipc_workspaces(&mut self) {
        let Some(server) = &mut self.ipc_server else {
            return;
        };

        // Accumulated events during this cycle
        let mut events = vec![];
        let mut compositor_state = server.compositor_state.borrow_mut();
        let ipc_workspaces = &mut compositor_state.workspaces;
        let mut active_workspace_id = None;

        for monitor in self.space.monitors() {
            let mon_active = monitor.active();

            let make_ipc_workspace = |workspace: &Workspace| {
                let mut current_windows: Vec<_> =
                    workspace.windows().map(Window::id).map(|id| *id).collect();
                current_windows.sort();

                fht_compositor_ipc::Workspace {
                    id: *workspace.id(),
                    output: workspace.output().name(),
                    windows: current_windows,
                    active_window_idx: workspace.active_tile_idx(),
                    fullscreen_window_idx: workspace.fullscreened_tile_idx(),
                    mwfact: workspace.mwfact(),
                    nmaster: workspace.nmaster(),
                }
            };

            for workspace in monitor.workspaces() {
                let entry = ipc_workspaces.entry(*workspace.id());

                entry
                    .and_modify(|ipc_workspace| {
                        if workspace_changed(ipc_workspace, workspace) {
                            *ipc_workspace = make_ipc_workspace(workspace);
                            events.push(Event::WorkspaceChanged(ipc_workspace.clone()));
                        }
                    })
                    .or_insert_with(|| {
                        let workspace = make_ipc_workspace(workspace);
                        events.push(Event::WorkspaceChanged(workspace.clone()));
                        workspace
                    });

                if mon_active && workspace.index() == monitor.active_idx {
                    active_workspace_id = Some(*workspace.id());
                }
            }
        }

        let active_workspace_id =
            active_workspace_id.expect("There should always be a focused workspace");
        if compositor_state.active_workspace_id != active_workspace_id {
            compositor_state.active_workspace_id = active_workspace_id;
            events.push(Event::ActiveWorkspaceChanged {
                id: active_workspace_id,
            });
        }

        drop(compositor_state); // rust argh!
        server.push_events(events);
    }
}

fn window_changed(window: &fht_compositor_ipc::Window, workspace_id: usize, tile: &Tile) -> bool {
    let location = tile.location() + tile.window_loc();
    let size = tile.window().size();

    // FIXME: The string comparaisons could be really expensive. Considering using (A)rc<str>
    tile.window().title() != window.title
        || tile.window().app_id() != window.app_id
        || tile.window().maximized() != window.maximized
        || tile.window().fullscreen() != window.fullscreened
        || tile.window().tiled() != window.tiled
        || workspace_id != window.workspace_id
        || window.location.0 != location.x
        || window.location.1 != location.y
        || window.size.0 != size.w as u32
        || window.size.1 != size.h as u32
}

fn workspace_changed(ipc_workspace: &fht_compositor_ipc::Workspace, workspace: &Workspace) -> bool {
    // NOTE: We don't check output name since it never changes
    let mut current_windows: Vec<_> = workspace.windows().map(Window::id).map(|id| *id).collect();
    // Sorting is required, since Vec::eq checks for equality element by element
    current_windows.sort();

    // We are doing some heuristics to avoid too much calculations, notably the (really expensive)
    // windows difference, which is a n^2 loop.
    //
    // The most likely change in a workspace is the active window index changing. Then comes the
    // fullscreen window changing, then the layout properties.
    workspace.active_tile_idx() != ipc_workspace.active_window_idx
        || workspace.fullscreened_tile_idx() != ipc_workspace.fullscreen_window_idx
        || workspace.mwfact() != ipc_workspace.mwfact
        || workspace.nmaster() != ipc_workspace.nmaster
        || ipc_workspace.windows != current_windows
}
