use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Context;
use async_channel::{Sender, TrySendError};
use calloop::io::Async;
use fht_compositor_ipc::Response;
use futures_util::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use smithay::input::pointer::{Focus, GrabStartData};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{
    Dispatcher, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
};
use smithay::reexports::rustix;
use smithay::reexports::rustix::fs::unlink;
use smithay::utils::{Point, Size, SERIAL_COUNTER};
use smithay::wayland::seat::WaylandFocus;

use crate::input::pick_surface_grab::{PickSurfaceGrab, PickSurfaceTarget};
use crate::input::KeyAction;
use crate::output::OutputExt;
use crate::space::WorkspaceId;
use crate::state::State;
use crate::utils::get_credentials_for_surface;
use crate::window::WindowId;

pub mod client;
mod subscribe;

/// The compositor IPC server.
pub struct Server {
    // The UnixSocket server that receives incoming clients
    listener_token: RegistrationToken,
    socket_path: PathBuf,
    compositor_state: Rc<RefCell<subscribe::CompositorState>>,
    subscribed_clients: Vec<Sender<fht_compositor_ipc::Event>>,
    dispatcher: Dispatcher<'static, Generic<UnixListener>, State>,
}

impl Server {
    pub fn close(self, loop_handle: &LoopHandle<'static, State>) {
        loop_handle.remove(self.listener_token);
        let _listener = Dispatcher::into_source_inner(self.dispatcher).unwrap();
        _ = unlink(self.socket_path);
    }

    pub fn push_events(
        &mut self,
        events: impl IntoIterator<Item = fht_compositor_ipc::Event> + 'static,
    ) {
        let mut to_disconnect = vec![];
        for event in events {
            for (idx, sender) in self.subscribed_clients.iter().enumerate() {
                match sender.try_send(event.clone()) {
                    Ok(()) => (),
                    Err(TrySendError::Full(_)) => {
                        // In this case for some reason the I/O on the client side is not reading
                        // events fast enough, so events are getting stuck in the (quite generous)
                        // event queue.
                        //
                        // I've noticed that this happens quite a lot with quickshell.
                        warn!("IPC client event channel is full, closing...");
                        to_disconnect.push(idx);
                    }
                    Err(TrySendError::Closed(_)) => {
                        // The channel is closed, disconnect basically. Nothing else todo.
                        error!("Failed to send event to subscribed client");
                        to_disconnect.push(idx);
                    }
                }
            }
        }

        to_disconnect.dedup();
        for idx in to_disconnect.into_iter().rev() {
            self.subscribed_clients.swap_remove(idx);
            // The client will automatically stop since it will exit out of the recv().await loop
        }
    }
}

/// Start the [`IpcServer`] for the compositor.
pub fn start(
    loop_handle: &LoopHandle<'static, State>,
    wayland_socket_name: &str,
) -> anyhow::Result<Server> {
    // First setup the communication channel between the IPC server and compositor
    let (to_compositor, from_clients) = calloop::channel::channel();
    loop_handle
        .insert_source(from_clients, |msg, _, state| {
            let calloop::channel::Event::Msg(req) = msg else {
                return;
            };

            if let Err(err) = state.handle_ipc_client_request(req) {
                error!(?err, "Failed to handle IPC client request");
            }
        })
        .map_err(|err| anyhow::anyhow!("Failed to insert calloop channel for IPC server: {err}"))?;

    let pid = std::process::id();

    // SAFETY: We place socket in XDG_RUNTIME_DIR, which should always be available to create the
    // wayland socket itself.
    let socket_dir = xdg::BaseDirectories::new().runtime_dir.unwrap();
    let socket_name = format!("fhtc-{pid}-{wayland_socket_name}.socket");
    let socket_path = socket_dir.join(&socket_name);
    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;

    let to_compositor_ = to_compositor.clone();
    let generic = Generic::new(listener, Interest::READ, Mode::Level);
    let dispatcher = Dispatcher::<_, State>::new(generic, move |_, listener, state| {
        match listener.accept() {
            Ok((socket, addr)) => {
                debug!(?addr, "New IPC client");

                // We want to make the socket driven by the event loop but have access to
                // asynchronous primitives We use calloop's Async to achieve exactly
                // this.

                let Ok(socket) = state
                    .fht
                    .loop_handle
                    .adapt_io(socket)
                    .inspect_err(|err| error!(?err, "Failed to create IPC client stream"))
                else {
                    return Ok(PostAction::Continue);
                };

                let Some(ipc_server) = &mut state.fht.ipc_server else {
                    unreachable!()
                };
                let scheduler = state.fht.scheduler.clone();
                let to_compositor = to_compositor_.clone();
                let compositor_state = ipc_server.compositor_state.clone();

                let fut = async move {
                    if let Err(err) =
                        handle_new_client(socket, to_compositor, compositor_state, scheduler).await
                    {
                        error!(?err, "Failed to handle IPC client");
                    }
                };

                state.fht.scheduler.schedule(fut).unwrap();
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => (),
            Err(err) => return Err(err),
        }

        Ok(PostAction::Continue)
    });
    let token = loop_handle.register_dispatcher(dispatcher.clone())?;

    unsafe {
        // SAFETY: We do not have any threaded activity **yet**
        std::env::set_var("FHTC_SOCKET_PATH", &socket_path);
    }

    info!(?socket_path, "Started IPC");

    Ok(Server {
        dispatcher,
        socket_path,
        compositor_state: Default::default(),
        subscribed_clients: vec![],
        listener_token: token,
    })
}

async fn handle_new_client(
    stream: Async<'static, UnixStream>,
    to_compositor: calloop::channel::Sender<ClientRequest>,
    compositor_state: Rc<RefCell<subscribe::CompositorState>>,
    scheduler: calloop::futures::Scheduler<()>,
) -> anyhow::Result<()> {
    crate::profile_function!();
    let (reader, mut writer) = stream.split();
    let mut reader = futures_util::io::BufReader::new(reader);

    // In fht-compositor IPC's model, each new line is a new request.
    // This allows the socket to be re-used to send out multiple requests

    let mut is_subscribe = false;
    loop {
        let mut req_buf = String::new();
        match reader.read_line(&mut req_buf).await {
            // Client disconnected. Thank you @Byson94 for spotting this!
            // Some clients send an empty buffer when disconnecting, which is, weird.
            Ok(0) => break,
            Ok(_) => (),
            // Client disconnected, stop this thread.
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            Err(err) => anyhow::bail!("error reading request: {err:?}"),
        }

        let request = serde_json::from_str::<fht_compositor_ipc::Request>(&req_buf);
        is_subscribe = matches!(request, Ok(fht_compositor_ipc::Request::Subscribe));

        // When you send a subscribe request to the socket, it can't be used anymore for regular
        // requests. This is a limitation of the current system, to avoid confusion
        // (how the client should interpret one of our responses?)
        //
        // FIXME: Handle other non-subscribe requests before? How should we tell when the client
        // stops sending other requests? I don't know, this is weird anyway. You should use
        // a separate socket for subscribing.
        if is_subscribe {
            break;
        }

        // We transform the Result::Err into a Response::Error
        let res = match request {
            Ok(req) => handle_request(req, to_compositor.clone(), compositor_state.clone())
                .await
                .map_err(|err| Response::Error(err.to_string()))
                .unwrap_or_else(std::convert::identity),
            Err(err) => Response::Error(err.to_string()),
        };

        let mut json = serde_json::to_string(&res)?;
        json.push('\n');
        if let Err(err) = writer.write_all(json.as_bytes()).await {
            warn!(?err, "Failed to write response to IPC client, closing...");
            break;
        }
    }

    if !is_subscribe {
        // nothing else to handle.
        return Ok(());
    }

    // If we do subscribe, we create a channel on which the compositor can inform us when state
    // changes. When there are any changes, we get pinged and do the diffing.
    let (event_tx, event_rx) = async_channel::unbounded();
    let fut = async move {
        if let Err(err) = subscribe::start_subscribing(event_rx, writer).await {
            warn!(?err, "Error during client subscription")
        }
    };
    scheduler.schedule(fut)?;
    to_compositor.send(ClientRequest::NewSubscriber(event_tx))?;

    Ok(())
}

enum ClientRequest {
    Outputs(async_channel::Sender<HashMap<String, fht_compositor_ipc::Output>>),
    PickWindow(async_channel::Sender<fht_compositor_ipc::PickWindowResult>),
    PickLayerShell(async_channel::Sender<fht_compositor_ipc::PickLayerShellResult>),
    CursorPosition(async_channel::Sender<(f64, f64)>),
    Action(
        fht_compositor_ipc::Action,
        async_channel::Sender<anyhow::Result<()>>,
    ),
    NewSubscriber(async_channel::Sender<fht_compositor_ipc::Event>),
}

async fn handle_request(
    req: fht_compositor_ipc::Request,
    to_compositor: calloop::channel::Sender<ClientRequest>,
    compositor_state: Rc<RefCell<subscribe::CompositorState>>,
) -> anyhow::Result<fht_compositor_ipc::Response> {
    match req {
        // The parent handle_client will do this for us
        fht_compositor_ipc::Request::Subscribe => Ok(Response::Noop),
        fht_compositor_ipc::Request::Version => {
            Ok(Response::Version(crate::cli::get_version_string()))
        }
        fht_compositor_ipc::Request::Outputs => {
            let (tx, rx) = async_channel::bounded(1);
            to_compositor
                .send(ClientRequest::Outputs(tx))
                .context("IPC communication channel closed")?;
            let outputs = rx
                .recv()
                .await
                .context("Failed to retreive output information")?;
            Ok(Response::Outputs(outputs))
        }
        fht_compositor_ipc::Request::Windows => {
            let windows = compositor_state.borrow().windows.clone();
            Ok(Response::Windows(windows))
        }
        fht_compositor_ipc::Request::LayerShells => {
            let layer_shells = compositor_state.borrow().layer_shells.clone();
            Ok(Response::LayerShells(layer_shells))
        }
        fht_compositor_ipc::Request::Space => {
            let space = compositor_state.borrow().space.clone();
            Ok(Response::Space(space))
        }
        fht_compositor_ipc::Request::Window(id) => {
            let windows = Ref::map(compositor_state.borrow(), |s| &s.windows);
            let window = windows.get(&id).cloned();
            Ok(Response::Window(window))
        }
        fht_compositor_ipc::Request::Workspace(id) => {
            let workspaces = Ref::map(compositor_state.borrow(), |s| &s.workspaces);
            let workspace = workspaces.get(&id).cloned();
            Ok(Response::Workspace(workspace))
        }
        fht_compositor_ipc::Request::GetWorkspace { output, index } => {
            let state = compositor_state.borrow();
            let id = match output {
                Some(name) => state
                    .space
                    .monitors
                    .get(&name)
                    .and_then(|mon| mon.workspaces.get(index))
                    .copied(),
                None => state
                    .space
                    .monitors
                    .values()
                    // if no output specified, use active.
                    .find(|mon| mon.active)
                    .and_then(|mon| mon.workspaces.get(index))
                    .copied(),
            };
            let workspace = id.and_then(|id| state.workspaces.get(&id)).cloned();

            Ok(Response::Workspace(workspace))
        }
        fht_compositor_ipc::Request::FocusedWindow => {
            let state = compositor_state.borrow();
            let window = state
                .focused_window_id
                .and_then(|id| state.windows.get(&id))
                .cloned();
            Ok(Response::Window(window))
        }
        fht_compositor_ipc::Request::FocusedWorkspace => {
            let state = compositor_state.borrow();
            let workspace = state.workspaces.get(&state.active_workspace_id).cloned();
            Ok(Response::Workspace(workspace))
        }
        fht_compositor_ipc::Request::PickWindow => {
            let (tx, rx) = async_channel::bounded(1024);
            to_compositor
                .send(ClientRequest::PickWindow(tx))
                .context("IPC communication channel closed")?;
            let result = rx.recv().await.context("Failed to receive picked window")?;
            Ok(Response::PickedWindow(result))
        }
        fht_compositor_ipc::Request::PickLayerShell => {
            let (tx, rx) = async_channel::bounded(1);
            to_compositor
                .send(ClientRequest::PickLayerShell(tx))
                .context("IPC communication channel closed")?;
            let result = rx
                .recv()
                .await
                .context("Failed to receive picked layer-shell")?;
            Ok(Response::PickedLayerShell(result))
        }
        fht_compositor_ipc::Request::CursorPosition => {
            let (tx, rx) = async_channel::bounded(1);
            to_compositor
                .send(ClientRequest::CursorPosition(tx))
                .context("IPC communication channel closed")?;
            let (x, y) = rx.recv().await.context("Failed to receive action result")?;
            Ok(Response::CursorPosition { x, y })
        }
        fht_compositor_ipc::Request::Action(action) => {
            let (tx, rx) = async_channel::bounded(1);
            to_compositor
                .send(ClientRequest::Action(action, tx))
                .context("IPC communication channel closed")?;
            let result = rx.recv().await.context("Failed to receive action result")?;
            let resp = match result {
                Ok(()) => Response::Noop,
                Err(err) => Response::Error(err.to_string()),
            };
            Ok(resp)
        }
    }
}

impl State {
    fn handle_ipc_client_request(&mut self, req: ClientRequest) -> anyhow::Result<()> {
        match req {
            ClientRequest::Outputs(tx) => {
                let outputs = self
                    .fht
                    .space
                    .outputs()
                    .map(|output| {
                        let name = output.name();
                        let props = output.physical_properties();
                        let preferred_mode = output.preferred_mode();
                        let active_mode = output.current_mode();
                        let mut active_mode_idx = None;

                        let modes = output
                            .modes()
                            .into_iter()
                            .enumerate()
                            .map(|(idx, mode)| {
                                if Some(mode) == active_mode {
                                    assert!(
                                        active_mode_idx.replace(idx).is_none(),
                                        "Two active modes on output"
                                    );
                                }

                                fht_compositor_ipc::OutputMode {
                                    dimensions: (mode.size.w as u32, mode.size.h as u32),
                                    preferred: Some(mode) == preferred_mode,
                                    refresh: mode.refresh as f64 / 1000.,
                                }
                            })
                            .collect();

                        let position = output.current_location().into();
                        let logical_size = output.geometry().size;
                        let scale = output.current_scale().integer_scale();
                        let transform = output.current_transform().into();

                        let ipc_output = fht_compositor_ipc::Output {
                            name: name.clone(),
                            make: props.make,
                            model: props.model,
                            serial: props.serial_number,
                            physical_size: Some((props.size.w as u32, props.size.h as u32)),
                            modes,
                            active_mode_idx,
                            position,
                            size: (logical_size.w as u32, logical_size.h as u32),
                            scale,
                            transform,
                        };

                        (name, ipc_output)
                    })
                    .collect();
                tx.send_blocking(outputs)?
            }
            ClientRequest::PickWindow(tx) => {
                let start_data = GrabStartData {
                    focus: None,
                    location: self.fht.pointer.current_location(),
                    button: 0,
                };
                // The previous grab will automatically be cancelled and the Cancelled result will
                // be sent when PickSurfaceGrab::unset handler is ran.
                let grab = PickSurfaceGrab {
                    target: PickSurfaceTarget::Window(tx),
                    start_data,
                };
                let pointer = self.fht.pointer.clone();
                pointer.set_grab(self, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
            }
            ClientRequest::PickLayerShell(tx) => {
                let start_data = GrabStartData {
                    focus: None,
                    location: self.fht.pointer.current_location(),
                    button: 0,
                };
                // The previous grab will automatically be cancelled and the Cancelled result will
                // be sent when PickSurfaceGrab::unset handler is ran.
                let grab = PickSurfaceGrab {
                    target: PickSurfaceTarget::LayerSurface(tx),
                    start_data,
                };
                let pointer = self.fht.pointer.clone();
                pointer.set_grab(self, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
            }
            ClientRequest::CursorPosition(tx) => {
                tx.send_blocking(self.fht.pointer.current_location().into())?;
            }
            ClientRequest::Action(action, tx) => {
                tx.send_blocking(self.handle_ipc_action(action))?;
            }
            ClientRequest::NewSubscriber(event_tx) => {
                let Some(ipc_server) = &mut self.fht.ipc_server else {
                    unreachable!()
                };

                let initial_state = ipc_server.compositor_state.borrow();
                let initial_events = [
                    fht_compositor_ipc::Event::Windows(initial_state.windows.clone()),
                    fht_compositor_ipc::Event::FocusedWindowChanged {
                        id: initial_state.focused_window_id,
                    },
                    fht_compositor_ipc::Event::Workspaces(initial_state.workspaces.clone()),
                    fht_compositor_ipc::Event::ActiveWorkspaceChanged {
                        id: initial_state.active_workspace_id,
                    },
                    fht_compositor_ipc::Event::Space(initial_state.space.clone()),
                    fht_compositor_ipc::Event::LayerShells(initial_state.layer_shells.clone()),
                ];

                // First broadcast initial state
                let tx_ = event_tx.clone();
                let fut = async move {
                    for event in initial_events {
                        if let Err(err) = tx_.send(event).await {
                            error!(?err, "Failed to send initial state to subscribed client");
                        }
                    }
                };

                // Then push the event sender into the ones we are updating
                ipc_server.subscribed_clients.push(event_tx);

                self.fht.scheduler.schedule(fut)?;
            }
        }

        Ok(())
    }

    fn handle_ipc_action(&mut self, action: fht_compositor_ipc::Action) -> anyhow::Result<()> {
        match action {
            fht_compositor_ipc::Action::Quit => self.fht.stop = true,
            fht_compositor_ipc::Action::DisableOutputs => self.fht.disable_outputs(),
            fht_compositor_ipc::Action::RunCommandLine { command_line } => {
                let (token, _token_data) =
                    self.fht.xdg_activation_state.create_external_token(None);
                crate::utils::spawn(command_line, Some(token.clone()));
            }
            fht_compositor_ipc::Action::Run { command } => {
                let (token, _token_data) =
                    self.fht.xdg_activation_state.create_external_token(None);
                crate::utils::spawn_args(command.clone(), Some(token.clone()));
            }
            fht_compositor_ipc::Action::ReloadConfig => self.reload_config(),
            fht_compositor_ipc::Action::SelectNextLayout { workspace_id } => {
                let workspace = match workspace_id {
                    Some(id) => self
                        .fht
                        .space
                        .workspace_mut_for_id(crate::space::WorkspaceId(id))
                        .context("No workspace with matching ID")?,
                    None => self.fht.space.active_workspace_mut(),
                };

                workspace.select_next_layout(true);
            }
            fht_compositor_ipc::Action::SelectPreviousLayout { workspace_id } => {
                let workspace = match workspace_id {
                    Some(id) => self
                        .fht
                        .space
                        .workspace_mut_for_id(crate::space::WorkspaceId(id))
                        .context("No workspace with matching ID")?,
                    None => self.fht.space.active_workspace_mut(),
                };

                workspace.select_previous_layout(true);
            }
            fht_compositor_ipc::Action::MaximizeWindow { state, window_id } => {
                let window = match window_id {
                    Some(id) => self
                        .fht
                        .space
                        .windows()
                        .find(|window| window.id() == WindowId(id))
                        .cloned()
                        .context("No window with matching ID")?,
                    // If there's no active window, we just silently return
                    None => {
                        if let Some(window) = self.fht.space.active_window() {
                            window
                        } else {
                            return Ok(());
                        }
                    }
                };

                let new_state = match state {
                    Some(s) => s,
                    None => !window.maximized(),
                };
                self.fht.space.maximize_window(&window, new_state, true);
            }
            fht_compositor_ipc::Action::FullscreenWindow { state, window_id } => {
                let window = match window_id {
                    Some(id) => self
                        .fht
                        .space
                        .windows()
                        .find(|window| window.id() == WindowId(id))
                        .cloned()
                        .context("No window with matching ID")?,
                    // If there's no active window, we just silently return
                    None => {
                        if let Some(window) = self.fht.space.active_window() {
                            window
                        } else {
                            return Ok(());
                        }
                    }
                };

                let new_state = match state {
                    Some(s) => s,
                    None => !window.fullscreen(),
                };
                if new_state {
                    self.fht.space.fullscreen_window(&window, true);
                } else {
                    window.request_fullscreen(false);
                }
            }
            fht_compositor_ipc::Action::FloatWindow { state, window_id } => {
                let window = match window_id {
                    Some(id) => self
                        .fht
                        .space
                        .windows()
                        .find(|window| window.id() == WindowId(id))
                        .cloned()
                        .context("No window with matching ID")?,
                    // If there's no active window, we just silently return
                    None => {
                        if let Some(window) = self.fht.space.active_window() {
                            window
                        } else {
                            return Ok(());
                        }
                    }
                };

                let new_state = match state {
                    Some(s) => s, /* we invert since we set whether the window is tiled, not */
                    // floating
                    None => window.tiled(),
                };
                self.fht.space.float_window(&window, new_state, true);
            }
            fht_compositor_ipc::Action::CenterFloatingWindow { window_id } => {
                let window = match window_id {
                    Some(id) => self
                        .fht
                        .space
                        .windows()
                        .find(|window| window.id() == WindowId(id))
                        .cloned()
                        .context("No window with matching ID")?,
                    None => {
                        if let Some(tile) = self.fht.space.active_window() {
                            tile
                        } else {
                            // If there's no active window, we just silently return
                            return Ok(());
                        }
                    }
                };

                if window.tiled() {
                    return Ok(());
                }

                self.fht.space.center_window(&window, true);
            }
            fht_compositor_ipc::Action::MoveFloatingWindow { window_id, change } => {
                let tile = match window_id {
                    Some(id) => self
                        .fht
                        .space
                        .tiles_mut()
                        .find(|tile| tile.window().id() == WindowId(id))
                        .context("No window with matching ID")?,
                    // If there's no active window, we just silently return
                    None => {
                        if let Some(window) = self.fht.space.active_tile_mut() {
                            window
                        } else {
                            return Ok(());
                        }
                    }
                };

                if tile.window().tiled() {
                    return Ok(());
                }

                let new_loc = match change {
                    fht_compositor_ipc::WindowLocationChange::Change { dx, dy } => {
                        let change = Point::from((dx.unwrap_or(0), dy.unwrap_or(0)));
                        tile.location() + change
                    }
                    fht_compositor_ipc::WindowLocationChange::Set { x, y } => {
                        let prev = tile.location();
                        Point::from((x.unwrap_or(prev.x), y.unwrap_or(prev.y)))
                    }
                };
                tile.set_location(new_loc, true);
            }
            fht_compositor_ipc::Action::ResizeFloatingWindow { window_id, change } => {
                let tile = match window_id {
                    Some(id) => self
                        .fht
                        .space
                        .tiles_mut()
                        .find(|tile| tile.window().id() == WindowId(id))
                        .context("No window with matching ID")?,
                    // If there's no active window, we just silently return
                    None => {
                        if let Some(window) = self.fht.space.active_tile_mut() {
                            window
                        } else {
                            return Ok(());
                        }
                    }
                };

                if tile.window().tiled() {
                    // FIXME: Figure out whether we should error or actually tell the user about
                    // the fact the window is not floating? Key-actions just ignore silently
                    return Ok(());
                }

                let new_size = match change {
                    fht_compositor_ipc::WindowSizeChange::Change { dx, dy } => {
                        let change = Size::from((dx.unwrap_or(0), dy.unwrap_or(0)));
                        tile.size() + change
                    }
                    fht_compositor_ipc::WindowSizeChange::Set { x, y } => {
                        let prev = tile.size();
                        Size::from((
                            x.unwrap_or(prev.w as u32) as i32,
                            y.unwrap_or(prev.h as u32) as i32,
                        ))
                    }
                };

                let new_size = Size::from((new_size.w.max(20), new_size.h.max(20)));
                tile.set_size(new_size, true);
            }
            fht_compositor_ipc::Action::FocusWindow { window_id } => {
                let window_id = WindowId(window_id);
                let mut window = None;

                for monitor in self.fht.space.monitors_mut() {
                    let mut workspace_idx = None;
                    for (ws_idx, workspace) in monitor.workspaces_mut().enumerate() {
                        let mut tile_idx = None;
                        if let Some((found_idx, tile)) = workspace
                            .tiles()
                            .enumerate()
                            .find(|(_, tile)| tile.window().id() == window_id)
                        {
                            window = Some(tile.window().clone());
                            tile_idx = Some(found_idx);
                        }

                        if let Some(idx) = tile_idx {
                            workspace.set_active_tile_idx(idx);
                            workspace.arrange_tiles(true);
                            workspace_idx = Some(ws_idx);
                            break;
                        }
                    }

                    if let Some(idx) = workspace_idx {
                        monitor.set_active_workspace_idx(idx, true);
                        break;
                    }
                }

                if let Some(window) = window {
                    self.set_keyboard_focus(Some(window));
                    return Ok(());
                }

                anyhow::bail!("No window with matching ID")
            }
            fht_compositor_ipc::Action::FocusNextWindow { workspace_id } => {
                let workspace = match workspace_id {
                    Some(id) => self
                        .fht
                        .space
                        .workspace_mut_for_id(crate::space::WorkspaceId(id))
                        .context("No workspace with matching ID")?,
                    None => self.fht.space.active_workspace_mut(),
                };

                workspace.activate_next_tile(true);
                self.update_keyboard_focus();
            }
            fht_compositor_ipc::Action::FocusPreviousWindow { workspace_id } => {
                let workspace = match workspace_id {
                    Some(id) => self
                        .fht
                        .space
                        .workspace_mut_for_id(crate::space::WorkspaceId(id))
                        .context("No workspace with matching ID")?,
                    None => self.fht.space.active_workspace_mut(),
                };

                workspace.activate_previous_tile(true);
                self.update_keyboard_focus();
            }
            fht_compositor_ipc::Action::SwapWithNextWindow {
                keep_focus,
                workspace_id,
            } => {
                let workspace = match workspace_id {
                    Some(id) => self
                        .fht
                        .space
                        .workspace_mut_for_id(crate::space::WorkspaceId(id))
                        .context("No workspace with matching ID")?,
                    None => self.fht.space.active_workspace_mut(),
                };

                workspace.swap_active_tile_with_next(keep_focus, true);
                self.update_keyboard_focus();
            }
            fht_compositor_ipc::Action::SwapWithPreviousWindow {
                keep_focus,
                workspace_id,
            } => {
                let workspace = match workspace_id {
                    Some(id) => self
                        .fht
                        .space
                        .workspace_mut_for_id(crate::space::WorkspaceId(id))
                        .context("No workspace with matching ID")?,
                    None => self.fht.space.active_workspace_mut(),
                };

                workspace.swap_active_tile_with_previous(keep_focus, true);
                self.update_keyboard_focus();
            }
            fht_compositor_ipc::Action::FocusOutput { output } => {
                let output = self
                    .fht
                    .space
                    .outputs()
                    .find(|o| o.name() == output)
                    .cloned()
                    .context("No output matching name")?;
                self.fht.focus_output(&output);
            }
            fht_compositor_ipc::Action::FocusNextOutput => {
                self.process_key_action(
                    KeyAction {
                        r#type: crate::input::KeyActionType::FocusNextOutput,
                        allow_while_locked: false,
                        repeat: false,
                    },
                    // We dont really care about the key pattern since its only used for
                    // key-repeating, which is turned off above.
                    Default::default(),
                );
            }
            fht_compositor_ipc::Action::FocusPreviousOutput => {
                self.process_key_action(
                    KeyAction {
                        r#type: crate::input::KeyActionType::FocusNextOutput,
                        allow_while_locked: false,
                        repeat: false,
                    },
                    // We dont really care about the key pattern since its only used for
                    // key-repeating, which is turned off above.
                    Default::default(),
                );
            }
            fht_compositor_ipc::Action::FocusWorkspace { workspace_id } => {
                let mut output = None;
                for monitor in self.fht.space.monitors_mut() {
                    let mut idx = None;
                    for (ws_idx, workspace) in monitor.workspaces().enumerate() {
                        if workspace.id() == WorkspaceId(workspace_id) {
                            idx = Some(ws_idx);
                            break;
                        }
                    }

                    if let Some(idx) = idx {
                        monitor.set_active_workspace_idx(idx, true);
                        output = Some(monitor.output().clone());
                        break;
                    }
                }

                let output = output.context("No workspace with matching ID")?;
                self.fht.focus_output(&output);
            }
            fht_compositor_ipc::Action::FocusWorkspaceByIndex {
                workspace_idx,
                output,
            } => {
                let monitor = match output {
                    None => self.fht.space.active_monitor_mut(),
                    Some(name) => self
                        .fht
                        .space
                        .monitors_mut()
                        .find(|mon| mon.output().name() == name)
                        .context("No output matching name")?,
                };

                anyhow::ensure!((0..9).contains(&workspace_idx), "Invalid workspace index");

                monitor.set_active_workspace_idx(workspace_idx, true);
                self.update_keyboard_focus();
            }
            fht_compositor_ipc::Action::FocusNextWorkspace { output } => {
                let monitor = match output {
                    None => self.fht.space.active_monitor_mut(),
                    Some(name) => self
                        .fht
                        .space
                        .monitors_mut()
                        .find(|mon| mon.output().name() == name)
                        .context("No output matching name")?,
                };

                let idx = (monitor.active_workspace_idx() + 1).clamp(0, 8);
                monitor.set_active_workspace_idx(idx, true);
                self.update_keyboard_focus();
            }
            fht_compositor_ipc::Action::FocusPreviousWorkspace { output } => {
                let monitor = match output {
                    None => self.fht.space.active_monitor_mut(),
                    Some(name) => self
                        .fht
                        .space
                        .monitors_mut()
                        .find(|mon| mon.output().name() == name)
                        .context("No output matching name")?,
                };

                let idx = monitor.active_workspace_idx().saturating_sub(1);
                monitor.set_active_workspace_idx(idx, true);
                self.update_keyboard_focus();
            }
            fht_compositor_ipc::Action::CloseWindow { window_id, kill } => {
                let window = match window_id {
                    Some(id) => self
                        .fht
                        .space
                        .windows()
                        .find(|window| window.id() == WindowId(id))
                        .cloned()
                        .context("No window with matching ID")?,
                    None => {
                        if let Some(tile) = self.fht.space.active_window() {
                            tile
                        } else {
                            // If there's no active window, we just silently return
                            return Ok(());
                        }
                    }
                };

                match kill {
                    false => window.toplevel().send_close(),
                    true => {
                        // Figure out the PID from credentials
                        let credentials =
                            get_credentials_for_surface(window.wl_surface().as_deref().unwrap())
                                .context("Failed to get wl_surface credentials")?;
                        rustix::process::kill_process(
                            rustix::process::Pid::from_raw(credentials.pid).unwrap(),
                            rustix::process::Signal::KILL,
                        )
                        .context("Failed to kill window process")?;
                    }
                }
            }
            fht_compositor_ipc::Action::ChangeMwfact {
                workspace_id,
                change,
            } => {
                let workspace = match workspace_id {
                    Some(id) => self
                        .fht
                        .space
                        .workspace_mut_for_id(crate::space::WorkspaceId(id))
                        .context("No workspace with matching ID")?,
                    None => self.fht.space.active_workspace_mut(),
                };

                match change {
                    fht_compositor_ipc::MwfactChange::Change { delta } => {
                        workspace.change_mwfact(delta, true)
                    }
                    fht_compositor_ipc::MwfactChange::Set { value } => {
                        workspace.set_mwfact(value, true)
                    }
                }
            }
            fht_compositor_ipc::Action::ChangeNmaster {
                workspace_id,
                change,
            } => {
                let workspace = match workspace_id {
                    Some(id) => self
                        .fht
                        .space
                        .workspace_mut_for_id(crate::space::WorkspaceId(id))
                        .context("No workspace with matching ID")?,
                    None => self.fht.space.active_workspace_mut(),
                };

                match change {
                    fht_compositor_ipc::NmasterChange::Change { delta } => {
                        workspace.change_nmaster(delta, true)
                    }
                    fht_compositor_ipc::NmasterChange::Set { value } => {
                        workspace.set_nmaster(value, true)
                    }
                }
            }
            fht_compositor_ipc::Action::ChangeWindowProportion { window_id, change } => {
                let tile = match window_id {
                    Some(id) => self
                        .fht
                        .space
                        .tiles_mut()
                        .find(|tile| tile.window().id() == WindowId(id))
                        .context("No window with matching ID")?,
                    // If there's no active window, we just silently return
                    None => {
                        if let Some(window) = self.fht.space.active_tile_mut() {
                            window
                        } else {
                            return Ok(());
                        }
                    }
                };

                match change {
                    fht_compositor_ipc::WindowProportionChange::Change { delta } => {
                        let new_value = tile.proportion() + delta;
                        tile.set_proportion(new_value);
                    }
                    fht_compositor_ipc::WindowProportionChange::Set { value } => {
                        tile.set_proportion(value)
                    }
                }
            }
            fht_compositor_ipc::Action::SendWindowToWorkspace {
                window_id,
                workspace_id,
            } => {
                let window = match window_id {
                    Some(id) => self
                        .fht
                        .space
                        .windows()
                        .find(|window| window.id() == WindowId(id))
                        .cloned()
                        .context("No window with matching ID")?,
                    None => {
                        if let Some(tile) = self.fht.space.active_window() {
                            tile
                        } else {
                            // If there's no active window, we just silently return
                            return Ok(());
                        }
                    }
                };

                self.fht
                    .space
                    .move_window_to_workspace(&window, WorkspaceId(workspace_id), true);
            }
        }

        Ok(())
    }
}
