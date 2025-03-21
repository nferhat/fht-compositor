use std::time::{Duration, SystemTime};

use anyhow::Result;
use async_channel::{Receiver, Sender};
use fhtctl::server::Server;
use fhtctl::{Command, Output, Response, ResponseData, Window as FhtctlWindow, Workspace};
use smithay::reexports::calloop;
use tracing::{info, warn};

use crate::state::State;

#[derive(Debug)]
pub(crate) enum IpcCommand {
    GetOutputs(async_channel::Sender<Vec<Output>>),
    GetWorkspaces(async_channel::Sender<Vec<Workspace>>),
    GetWindows(async_channel::Sender<Vec<FhtctlWindow>>),
    FocusWindow {
        app_id: Option<String>,
        title: Option<String>,
        response_sender: async_channel::Sender<Result<(), String>>,
    },
    CloseWindow(async_channel::Sender<Result<(), String>>),
    SwitchWorkspace {
        id: usize,
        response_sender: async_channel::Sender<Result<(), String>>,
    },
}

pub struct IpcState {
    #[allow(dead_code)]
    command_sender: Sender<IpcCommand>,
    server_control: Option<fhtctl::server::ServerControl>,
}

impl IpcState {
    pub fn new(command_sender: Sender<IpcCommand>) -> Self {
        Self {
            command_sender,
            server_control: None,
        }
    }
}

pub fn init_ipc_server(state: &mut State) -> Result<()> {
    info!("Initializing IPC server");

    let (command_sender, command_receiver) = async_channel::unbounded::<IpcCommand>();

    if state.fht.ipc_state.is_none() {
        state.fht.ipc_state = Some(IpcState::new(command_sender.clone()));
    }

    if let Err(err) = setup_command_handler(state, command_receiver) {
        warn!(?err, "Failed to set up IPC command handler");
        return Err(err);
    }

    let mut server = Server::new();
    let start_time = SystemTime::now();

    server.register_handler(Command::GetState, move |_| {
        let version = std::env!("CARGO_PKG_VERSION").to_string();
        let uptime = SystemTime::now()
            .duration_since(start_time)
            .unwrap_or(Duration::from_secs(0))
            .as_secs();

        Response::success(Some(ResponseData::State { version, uptime }))
    });

    let cmd_sender = command_sender.clone();
    server.register_handler(Command::GetOutputs, move |_| {
        let (response_sender, response_receiver) = async_channel::bounded(1);

        if let Err(e) = cmd_sender.try_send(IpcCommand::GetOutputs(response_sender)) {
            return Response::error(format!("Failed to send command: {}", e));
        }

        match response_receiver.recv_blocking() {
            Ok(outputs) => Response::success(Some(ResponseData::Outputs(outputs))),
            Err(e) => Response::error(format!("Failed to receive response: {}", e)),
        }
    });

    let cmd_sender = command_sender.clone();
    server.register_handler(Command::GetWorkspaces, move |_| {
        let (response_sender, response_receiver) = async_channel::bounded(1);

        if let Err(e) = cmd_sender.try_send(IpcCommand::GetWorkspaces(response_sender)) {
            return Response::error(format!("Failed to send command: {}", e));
        }

        match response_receiver.recv_blocking() {
            Ok(workspaces) => Response::success(Some(ResponseData::Workspaces(workspaces))),
            Err(e) => Response::error(format!("Failed to receive response: {}", e)),
        }
    });

    let cmd_sender = command_sender.clone();
    server.register_handler(Command::GetWindows, move |_| {
        let (response_sender, response_receiver) = async_channel::bounded(1);

        if let Err(e) = cmd_sender.try_send(IpcCommand::GetWindows(response_sender)) {
            return Response::error(format!("Failed to send command: {}", e));
        }

        match response_receiver.recv_blocking() {
            Ok(windows) => Response::success(Some(ResponseData::Windows(windows))),
            Err(e) => Response::error(format!("Failed to receive response: {}", e)),
        }
    });

    let cmd_sender = command_sender.clone();
    server.register_handler(
        Command::FocusWindow {
            app_id: None,
            title: None,
        },
        move |cmd| {
            let (app_id, title) = if let Command::FocusWindow { app_id, title } = cmd {
                (app_id, title)
            } else {
                return Response::error("Invalid command");
            };

            let (response_sender, response_receiver) = async_channel::bounded(1);

            if let Err(e) = cmd_sender.try_send(IpcCommand::FocusWindow {
                app_id,
                title,
                response_sender,
            }) {
                return Response::error(format!("Failed to send command: {}", e));
            }

            match response_receiver.recv_blocking() {
                Ok(Ok(())) => Response::success(None),
                Ok(Err(e)) => Response::error(e),
                Err(e) => Response::error(format!("Failed to receive response: {}", e)),
            }
        },
    );

    let cmd_sender = command_sender.clone();
    server.register_handler(Command::CloseWindow, move |_| {
        let (response_sender, response_receiver) = async_channel::bounded(1);

        if let Err(e) = cmd_sender.try_send(IpcCommand::CloseWindow(response_sender)) {
            return Response::error(format!("Failed to send command: {}", e));
        }

        match response_receiver.recv_blocking() {
            Ok(Ok(())) => Response::success(None),
            Ok(Err(e)) => Response::error(e),
            Err(e) => Response::error(format!("Failed to receive response: {}", e)),
        }
    });

    let cmd_sender = command_sender.clone();
    server.register_handler(Command::SwitchWorkspace { id: 0 }, move |cmd| {
        let id = if let Command::SwitchWorkspace { id } = cmd {
            id
        } else {
            return Response::error("Invalid command");
        };

        let (response_sender, response_receiver) = async_channel::bounded(1);

        if let Err(e) = cmd_sender.try_send(IpcCommand::SwitchWorkspace {
            id,
            response_sender,
        }) {
            return Response::error(format!("Failed to send command: {}", e));
        }

        match response_receiver.recv_blocking() {
            Ok(Ok(())) => Response::success(None),
            Ok(Err(e)) => Response::error(e),
            Err(e) => Response::error(format!("Failed to receive response: {}", e)),
        }
    });

    match server.start() {
        Ok(server_control) => {
            if let Some(ipc_state) = &mut state.fht.ipc_state {
                ipc_state.server_control = Some(server_control);
            }
            info!("IPC server started");
            Ok(())
        }
        Err(e) => {
            warn!("Failed to start IPC server: {}", e);
            Err(e.into())
        }
    }
}

fn setup_command_handler(state: &mut State, command_receiver: Receiver<IpcCommand>) -> Result<()> {
    let (tx, rx) = calloop::channel::channel();

    std::thread::Builder::new()
        .name("ipc-bridge".into())
        .spawn(move || {
            while let Ok(cmd) = command_receiver.recv_blocking() {
                if tx.send(cmd).is_err() {
                    break;
                }
            }
        })?;

    state
        .fht
        .loop_handle
        .insert_source(rx, move |event, _, state| {
            if let calloop::channel::Event::Msg(cmd) = event {
                handle_ipc_command(cmd, state);
            }
        })
        .map_err(|err| anyhow::anyhow!("Failed to insert IPC command handler: {:?}", err))?;

    Ok(())
}

fn handle_ipc_command(command: IpcCommand, state: &mut State) {
    match command {
        IpcCommand::GetOutputs(response_sender) => {
            let outputs = state
                .fht
                .space
                .outputs()
                .map(|output| {
                    let physical_props = output.physical_properties();
                    Output {
                        name: output.name(),
                        make: physical_props.make.clone(),
                        model: physical_props.model.clone(),
                        width: output.current_mode().map(|m| m.size.w).unwrap_or_default() as u32,
                        height: output.current_mode().map(|m| m.size.h).unwrap_or_default() as u32,
                        refresh_rate: output.current_mode().map(|m| m.refresh).unwrap_or_default()
                            as f64
                            / 1000.0,
                        active: true,
                    }
                })
                .collect();

            let _ = response_sender.try_send(outputs);
        }

        IpcCommand::GetWorkspaces(response_sender) => {
            let mut workspaces = Vec::new();
            let active_workspace_id = state.fht.space.active_workspace_id();

            for monitor in state.fht.space.monitors() {
                for workspace in monitor.workspaces() {
                    let output = monitor.output().name();
                    let window_count = workspace.windows().count();
                    let id = workspace.id();

                    workspaces.push(Workspace {
                        id: id.0 as usize,
                        name: format!("Workspace {}", id.0 + 1),
                        active: id == active_workspace_id,
                        output,
                        window_count,
                    });
                }
            }

            let _ = response_sender.try_send(workspaces);
        }

        IpcCommand::GetWindows(response_sender) => {
            let mut windows = Vec::new();
            let active_window = state.fht.space.active_window();

            for monitor in state.fht.space.monitors() {
                for workspace in monitor.workspaces() {
                    for window in workspace.windows() {
                        windows.push(FhtctlWindow {
                            app_id: window.app_id(),
                            title: window.title(),
                            workspace_id: workspace.id().0 as usize,
                            focused: active_window
                                .as_ref()
                                .map_or(false, |w| w.id() == window.id()),
                            maximized: window.maximized(),
                            fullscreen: window.fullscreen(),
                            pid: None,
                        });
                    }
                }
            }

            let _ = response_sender.try_send(windows);
        }

        IpcCommand::FocusWindow {
            app_id,
            title,
            response_sender,
        } => {
            let mut target_output = None;
            let mut target_workspace_idx = None;

            for monitor in state.fht.space.monitors() {
                for (ws_idx, workspace) in monitor.workspaces().enumerate() {
                    for window in workspace.windows() {
                        let matches_app_id = app_id.as_ref().map_or(true, |id| {
                            window.app_id().is_some_and(|window_id| window_id == *id)
                        });

                        let matches_title = title.as_ref().map_or(true, |t| {
                            window
                                .title()
                                .is_some_and(|window_title| window_title == *t)
                        });

                        if matches_app_id && matches_title {
                            target_output = Some(monitor.output().clone());
                            target_workspace_idx = Some(ws_idx);
                            break;
                        }
                    }
                    if target_output.is_some() {
                        break;
                    }
                }
                if target_output.is_some() {
                    break;
                }
            }

            if let (Some(output), Some(ws_idx)) = (target_output, target_workspace_idx) {
                state.fht.space.set_active_output(&output);

                let active_monitor = state.fht.space.active_monitor_mut();
                active_monitor.set_active_workspace_idx(ws_idx, true);

                let _ = response_sender.try_send(Ok(()));
            } else {
                let _ = response_sender.try_send(Err("Window not found".into()));
            }
        }

        IpcCommand::CloseWindow(response_sender) => {
            if let Some(window) = state.fht.space.active_window() {
                let toplevel = window.toplevel();
                toplevel.send_close();
                let _ = response_sender.try_send(Ok(()));
            } else {
                let _ = response_sender.try_send(Err("No active window to close".into()));
            }
        }

        IpcCommand::SwitchWorkspace {
            id,
            response_sender,
        } => {
            let mut target_output = None;
            let mut target_idx = None;

            for monitor in state.fht.space.monitors() {
                for (idx, workspace) in monitor.workspaces().enumerate() {
                    if workspace.id().0 == id {
                        target_output = Some(monitor.output().clone());
                        target_idx = Some(idx);
                        break;
                    }
                }
                if target_output.is_some() {
                    break;
                }
            }

            if let (Some(output), Some(idx)) = (target_output, target_idx) {
                state.fht.space.set_active_output(&output);

                let active_monitor = state.fht.space.active_monitor_mut();
                active_monitor.set_active_workspace_idx(idx, true);

                let _ = response_sender.try_send(Ok(()));
            } else {
                let _ = response_sender.try_send(Err(format!("Workspace {} not found", id)));
            }
        }
    }
}
