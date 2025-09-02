use std::collections::HashMap;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use calloop::io::Async;
use fht_compositor_ipc::Response;
use futures_util::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use once_cell::sync::Lazy;
use smithay::desktop::layer_map_for_output;
use smithay::input::pointer::{Focus, GrabStartData};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{
    Dispatcher, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
};
use smithay::reexports::rustix;
use smithay::utils::{Point, Size, SERIAL_COUNTER};
use smithay::wayland::seat::WaylandFocus;

use crate::focus_target::KeyboardFocusTarget;
use crate::input::pick_surface_grab::{PickSurfaceGrab, PickSurfaceTarget};
use crate::input::KeyAction;
use crate::output::OutputExt;
use crate::space::{Workspace, WorkspaceId};
use crate::state::State;
use crate::utils::get_credentials_for_surface;
use crate::window::{Window, WindowId};

pub mod client;

/// The compositor IPC server.
pub struct Server {
    // The UnixSocket server that receives incoming clients
    listener_token: RegistrationToken,
    dispatcher: Dispatcher<'static, Generic<UnixListener>, State>,
}

impl Server {
    pub fn close(self, loop_handle: &LoopHandle<'static, State>) {
        loop_handle.remove(self.listener_token);
        let _listener = Dispatcher::into_source_inner(self.dispatcher).unwrap();
        // FIXME: Close socket?
    }
}

/// [`SubscriberSnapshots`] Holds the previous state of the data send to subscribers.
#[derive(Default)]
struct SubscriberSnapshots {
    windows: Option<HashMap<usize, fht_compositor_ipc::Window>>,
    workspaces: HashMap<usize, Option<fht_compositor_ipc::Workspace>>,
    space: Option<fht_compositor_ipc::Space>,
    layer_shells: Option<Vec<fht_compositor_ipc::LayerShell>>,
}

/// [`IpcServerSubscriberState`] Handles all the ipc stuff that we can subscribe to.
pub struct SubscriberManager {
    subscribers_windows: Vec<async_channel::Sender<HashMap<usize, fht_compositor_ipc::Window>>>,
    subscribers_space: Vec<async_channel::Sender<fht_compositor_ipc::Space>>,
    subscribers_layer_shells: Vec<async_channel::Sender<Vec<fht_compositor_ipc::LayerShell>>>,
    subscribers_window:
        HashMap<usize, Vec<async_channel::Sender<Option<fht_compositor_ipc::Window>>>>,
    subscribers_workspace:
        HashMap<usize, Vec<async_channel::Sender<Option<fht_compositor_ipc::Workspace>>>>,

    snapshots: SubscriberSnapshots,
}

// GLOBAL
pub static IPC_SUB_STATE: Lazy<Arc<Mutex<SubscriberManager>>> =
    Lazy::new(|| Arc::new(Mutex::new(SubscriberManager::new())));

// Wrapper that broadcast to clients by diffing
pub fn try_broadcast_from_global(state: &State) {
    if let Ok(mut sub_mgr) = crate::ipc::IPC_SUB_STATE.lock() {
        sub_mgr.diff_and_update(state);
    }
}

impl SubscriberManager {
    pub fn new() -> Self {
        Self {
            subscribers_windows: Vec::new(),
            subscribers_space: Vec::new(),
            subscribers_layer_shells: Vec::new(),
            subscribers_window: HashMap::new(),
            subscribers_workspace: HashMap::new(),
            snapshots: SubscriberSnapshots::default(),
        }
    }

    /// Picks the data needed for subscribers, diffs it against previous snapshot,
    /// and broadcasts only changes.
    pub fn diff_and_update(&mut self, state: &State) {
        // == Windows ==
        if !self.subscribers_windows.is_empty() || !self.subscribers_window.is_empty() {
            let all_windows: Vec<fht_compositor_ipc::Window> = state
                .fht
                .space
                .monitors()
                .flat_map(|mon| {
                    mon.workspaces().flat_map(|ws| {
                        ws.tiles().map(|tile| {
                            let window = tile.window();
                            let location = tile.location() + tile.window_loc();
                            let size = window.size();
                            fht_compositor_ipc::Window {
                                id: *window.id(),
                                title: window.title(),
                                app_id: window.app_id(),
                                output: ws.output().name(),
                                workspace_idx: ws.index(),
                                workspace_id: *ws.id(),
                                size: (size.w as u32, size.h as u32),
                                location: location.into(),
                                fullscreened: window.fullscreen(),
                                maximized: window.maximized(),
                                tiled: window.tiled(),
                                activated: true,
                                focused: true,
                            }
                        })
                    })
                })
                .collect();

            let window_map: HashMap<usize, _> =
                all_windows.iter().map(|w| (w.id, w.clone())).collect();

            // Send only if different
            if self.snapshots.windows.as_ref() != Some(&window_map) {
                self.snapshots.windows = Some(window_map.clone());
                for s in &self.subscribers_windows {
                    let _ = s.try_send(window_map.clone());
                }
                for (&id, subs) in &self.subscribers_window {
                    if let Some(window) = window_map.get(&id) {
                        for s in subs {
                            let _ = s.try_send(Some(window.clone()));
                        }
                    }
                }
            }
        }

        // == Workspaces ==
        if !self.subscribers_workspace.is_empty() {
            for (&id, subs) in &self.subscribers_workspace {
                let workspace = if id == usize::MAX {
                    Some(state.fht.space.active_workspace())
                } else {
                    state.fht.space.workspace_for_id(WorkspaceId(id))
                }
                .map(|ws| fht_compositor_ipc::Workspace {
                    output: ws.output().name(),
                    id: *ws.id(),
                    active_window_idx: ws.active_tile_idx(),
                    fullscreen_window_idx: ws.fullscreened_tile_idx(),
                    mwfact: ws.mwfact(),
                    nmaster: ws.nmaster(),
                    windows: ws.windows().map(|w| *w.id()).collect(),
                });

                let send = self.snapshots.workspaces.get(&id) != Some(&workspace);
                if send {
                    self.snapshots.workspaces.insert(id, workspace.clone());
                    for s in subs {
                        let _ = s.try_send(workspace.clone());
                    }
                }
            }
        }

        // == Space ==
        if !self.subscribers_space.is_empty() {
            let monitors = state
                .fht
                .space
                .monitors()
                .map(|mon| {
                    let workspaces: [fht_compositor_ipc::Workspace; 9] = mon
                        .workspaces()
                        .map(|ws| fht_compositor_ipc::Workspace {
                            output: mon.output().name(),
                            id: *ws.id(),
                            active_window_idx: ws.active_tile_idx(),
                            fullscreen_window_idx: ws.fullscreened_tile_idx(),
                            mwfact: ws.mwfact(),
                            nmaster: ws.nmaster(),
                            windows: ws.windows().map(|w| *w.id()).collect(),
                        })
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
                active_idx: state.fht.space.active_monitor_idx(),
                primary_idx: state.fht.space.primary_monitor_idx(),
            };

            if self.snapshots.space.as_ref() != Some(&space) {
                self.snapshots.space = Some(space.clone());
                for s in &self.subscribers_space {
                    let _ = s.try_send(space.clone());
                }
            }
        }

        // == Layer shells ==
        if !self.subscribers_layer_shells.is_empty() {
            let mut layers = Vec::new();
            for output in state.fht.space.outputs() {
                let layer_map = layer_map_for_output(output);
                let output_name = output.name();
                for layer_surface in layer_map.layers() {
                    layers.push(fht_compositor_ipc::LayerShell {
                        namespace: layer_surface.namespace().to_string(),
                        output: output_name.clone(),
                        #[allow(clippy::missing_transmute_annotations)]
                        layer: unsafe { std::mem::transmute(layer_surface.layer()) },
                        #[allow(clippy::missing_transmute_annotations)]
                        keyboard_interactivity: unsafe {
                            std::mem::transmute(layer_surface.cached_state().keyboard_interactivity)
                        },
                    });
                }
            }

            if self.snapshots.layer_shells.as_ref() != Some(&layers) {
                self.snapshots.layer_shells = Some(layers.clone());
                for s in &self.subscribers_layer_shells {
                    let _ = s.try_send(layers.clone());
                }
            }
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
        .insert_source(from_clients, move |msg, _, state| {
            // THIS thing only triggers if there is a msg
            {
                try_broadcast_from_global(&state);
            }

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

                let scheduler = state.fht.scheduler.clone();
                let to_compositor = to_compositor_.clone();
                let fut = async move {
                    if let Err(err) = handle_new_client(socket, to_compositor, scheduler).await {
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
        listener_token: token,
    })
}

async fn handle_new_client(
    stream: Async<'static, UnixStream>,
    to_compositor: calloop::channel::Sender<ClientRequest>,
    scheduler: calloop::futures::Scheduler<()>,
) -> anyhow::Result<()> {
    crate::profile_function!();
    let (reader, mut writer) = stream.split();
    let mut reader = futures_util::io::BufReader::new(reader);

    // In fht-compositor IPC's model, each new line is a new request.
    // This allows the socket to be re-used to send out multiple requests

    // for handling subscriptions
    let (tx, rx) = async_channel::unbounded::<fht_compositor_ipc::Response>();

    // write stuff when tx gets a request
    scheduler
        .schedule(async move {
            while let Ok(response) = rx.recv().await {
                match serde_json::to_string(&response) {
                    Ok(mut s) => {
                        s.push('\n');
                        if writer.write_all(s.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .unwrap();

    loop {
        let mut req_buf = String::new();
        match reader.read_line(&mut req_buf).await {
            // For future developers,
            // FOR GODS SAKE, PLEASE DONT REMOVE THIS EOF LINE!
            // If you did, there will be massive concequences!
            //
            // DONT TELL ME THAT I DIDNT WARN YOU, IT IS YOUR DEVICE THAT WILL SUFFER!
            Ok(0) => {
                // EOF
                break Ok(());
            }
            Ok(_) => (),
            Err(err) => {
                if matches!(
                    err.kind(),
                    io::ErrorKind::ConnectionAborted | io::ErrorKind::BrokenPipe
                ) {
                    // the socket closed, stop this client thread
                    return Ok(());
                } else {
                    // Otherwise, some sort of error happened
                    anyhow::bail!("error reading request: {err:?}")
                }
            }
        }

        let request = serde_json::from_str::<fht_compositor_ipc::IpcRequest>(&req_buf);

        // We transform the Result::Err into a Response::Error
        match request {
            Ok(req) => {
                if let Err(err) =
                    handle_request(req, to_compositor.clone(), tx.clone(), scheduler.clone()).await
                {
                    let _ = tx.send(Response::Error(err.to_string())).await;
                }
            }
            Err(err) => {
                let _ = tx.send(Response::Error(err.to_string())).await;
            }
        }
    }
}

enum ClientRequest {
    Outputs(async_channel::Sender<HashMap<String, fht_compositor_ipc::Output>>),
    Windows(async_channel::Sender<HashMap<usize, fht_compositor_ipc::Window>>),
    LayerShells(async_channel::Sender<Vec<fht_compositor_ipc::LayerShell>>),
    Space(async_channel::Sender<fht_compositor_ipc::Space>),
    Window {
        /// When `id` is `None`, request the focused window.
        id: Option<usize>,
        sender: async_channel::Sender<Option<fht_compositor_ipc::Window>>,
    },
    Workspace {
        /// When `id` is `None`, request the focused workspace.
        id: Option<usize>,
        sender: async_channel::Sender<Option<fht_compositor_ipc::Workspace>>,
    },
    WorkspaceByIndex {
        /// When `output` is `None`, use the focused output.
        output: Option<String>,
        index: usize,
        sender: async_channel::Sender<Option<fht_compositor_ipc::Workspace>>,
    },
    PickWindow(async_channel::Sender<fht_compositor_ipc::PickWindowResult>),
    PickLayerShell(async_channel::Sender<fht_compositor_ipc::PickLayerShellResult>),
    Action(
        fht_compositor_ipc::Action,
        async_channel::Sender<anyhow::Result<()>>,
    ),

    // subscription variants
    SubscribeWindows(async_channel::Sender<HashMap<usize, fht_compositor_ipc::Window>>),
    SubscribeSpace(async_channel::Sender<fht_compositor_ipc::Space>),
    SubscribeLayerShells(async_channel::Sender<Vec<fht_compositor_ipc::LayerShell>>),
    SubscribeWindow {
        /// When `id` is `None`, request the focused workspace.
        id: Option<usize>,
        sender: async_channel::Sender<Option<fht_compositor_ipc::Window>>,
    },
    SubscribeWorkspace {
        /// When `id` is `None`, request the focused workspace.
        id: Option<usize>,
        sender: async_channel::Sender<Option<fht_compositor_ipc::Workspace>>,
    },
}

async fn handle_request(
    req: fht_compositor_ipc::IpcRequest,
    to_compositor: calloop::channel::Sender<ClientRequest>,
    tx: async_channel::Sender<fht_compositor_ipc::Response>,
    scheduler: calloop::futures::Scheduler<()>,
) -> anyhow::Result<()> {
    if req.subscribe {
        match req.request {
            fht_compositor_ipc::Request::Windows => {
                let (sub_tx, sub_rx) = async_channel::unbounded();
                to_compositor.send(ClientRequest::SubscribeWindows(sub_tx))?;

                let rx = sub_rx.clone();
                let tx_clone = tx.clone();
                scheduler
                    .schedule(async move {
                        while let Ok(update) = rx.recv().await {
                            let _ = tx_clone.send(Response::Windows(update)).await;
                        }
                    })
                    .unwrap();
            }

            fht_compositor_ipc::Request::Space => {
                let (sub_tx, sub_rx) = async_channel::unbounded();
                to_compositor.send(ClientRequest::SubscribeSpace(sub_tx))?;

                let rx = sub_rx.clone();
                let tx_clone = tx.clone();
                scheduler
                    .schedule(async move {
                        while let Ok(update) = rx.recv().await {
                            let _ = tx_clone.send(Response::Space(update)).await;
                        }
                    })
                    .unwrap();
            }

            fht_compositor_ipc::Request::Workspace(id) => {
                let (sub_tx, sub_rx) = async_channel::unbounded();
                to_compositor.send(ClientRequest::SubscribeWorkspace {
                    id: Some(id),
                    sender: sub_tx,
                })?;

                let rx = sub_rx.clone();
                let tx_clone = tx.clone();
                scheduler
                    .schedule(async move {
                        while let Ok(update) = rx.recv().await {
                            let _ = tx_clone.send(Response::Workspace(update)).await;
                        }
                    })
                    .unwrap();
            }

            fht_compositor_ipc::Request::LayerShells => {
                let (sub_tx, sub_rx) = async_channel::unbounded();
                to_compositor.send(ClientRequest::SubscribeLayerShells(sub_tx))?;

                let rx = sub_rx.clone();
                let tx_clone = tx.clone();
                scheduler
                    .schedule(async move {
                        while let Ok(update) = rx.recv().await {
                            let _ = tx_clone.send(Response::LayerShells(update)).await;
                        }
                    })
                    .unwrap();
            }

            fht_compositor_ipc::Request::Window(id) => {
                let (sub_tx, sub_rx) = async_channel::unbounded();
                to_compositor.send(ClientRequest::SubscribeWindow {
                    id: Some(id),
                    sender: sub_tx,
                })?;

                let rx = sub_rx.clone();
                let tx_clone = tx.clone();
                scheduler
                    .schedule(async move {
                        while let Ok(update) = rx.recv().await {
                            let _ = tx_clone.send(Response::Window(update)).await;
                        }
                    })
                    .unwrap();
            }

            _ => {
                let tx_clone = tx.clone();
                let _ = tx_clone
                    .send(Response::Error(
                        "This request cannot be subscribed to.".to_string(),
                    ))
                    .await;
            }
        }
    } else {
        let res = match req.request {
            fht_compositor_ipc::Request::Version => {
                Response::Version(crate::cli::get_version_string())
            }
            fht_compositor_ipc::Request::Outputs => {
                let (atx, arx) = async_channel::bounded(1);
                to_compositor
                    .send(ClientRequest::Outputs(atx))
                    .context("IPC communication channel closed")?;
                let outputs = arx
                    .recv()
                    .await
                    .context("Failed to retreive output information")?;

                Response::Outputs(outputs)
            }
            fht_compositor_ipc::Request::Windows => {
                let (atx, arx) = async_channel::bounded(1);
                to_compositor
                    .send(ClientRequest::Windows(atx))
                    .context("IPC communication channel closed")?;
                let windows = arx
                    .recv()
                    .await
                    .context("Failed to retreive output information")?;

                Response::Windows(windows)
            }
            fht_compositor_ipc::Request::LayerShells => {
                let (atx, arx) = async_channel::bounded(1);
                to_compositor
                    .send(ClientRequest::LayerShells(atx))
                    .context("IPC communication channel closed")?;
                let layer_shells = arx
                    .recv()
                    .await
                    .context("Failed to retreive layer-shell information")?;

                Response::LayerShells(layer_shells)
            }
            fht_compositor_ipc::Request::Space => {
                let (atx, arx) = async_channel::bounded(1);
                to_compositor
                    .send(ClientRequest::Space(atx))
                    .context("IPC communication channel closed")?;
                let space = arx
                    .recv()
                    .await
                    .context("Failed to retreive output information")?;

                Response::Space(space)
            }
            fht_compositor_ipc::Request::Window(id) => {
                let (atx, arx) = async_channel::bounded(1);
                let req = ClientRequest::Window {
                    id: Some(id),
                    sender: atx,
                };
                to_compositor
                    .send(req)
                    .context("IPC communication channel closed")?;
                let space = arx
                    .recv()
                    .await
                    .context("Failed to retreive focused window information")?;

                Response::Window(space)
            }
            fht_compositor_ipc::Request::Workspace(id) => {
                let (atx, arx) = async_channel::bounded(1);
                let req = ClientRequest::Workspace {
                    id: Some(id),
                    sender: atx,
                };
                to_compositor
                    .send(req)
                    .context("IPC communication channel closed")?;
                let workspace = arx
                    .recv()
                    .await
                    .context("Failed to retreive focused workspace information")?;

                Response::Workspace(workspace)
            }
            fht_compositor_ipc::Request::GetWorkspace { output, index } => {
                let (atx, arx) = async_channel::bounded(1);
                let req = ClientRequest::WorkspaceByIndex {
                    output,
                    index,
                    sender: atx,
                };

                to_compositor
                    .send(req)
                    .context("IPC communication channel closed")?;
                let workspace = arx
                    .recv()
                    .await
                    .context("Failed to retreive focused workspace information")?;

                Response::Workspace(workspace)
            }
            fht_compositor_ipc::Request::FocusedWindow => {
                let (atx, arx) = async_channel::bounded(1);
                let req = ClientRequest::Window {
                    id: None,
                    sender: atx,
                };
                to_compositor
                    .send(req)
                    .context("IPC communication channel closed")?;
                let space = arx
                    .recv()
                    .await
                    .context("Failed to retreive focused window information")?;

                Response::Window(space)
            }
            fht_compositor_ipc::Request::FocusedWorkspace => {
                let (atx, arx) = async_channel::bounded(1);
                let req = ClientRequest::Workspace {
                    id: None,
                    sender: atx,
                };
                to_compositor
                    .send(req)
                    .context("IPC communication channel closed")?;
                let workspace = arx
                    .recv()
                    .await
                    .context("Failed to retreive focused workspace information")?;

                Response::Workspace(workspace)
            }
            fht_compositor_ipc::Request::PickWindow => {
                let (atx, arx) = async_channel::bounded(1024);
                to_compositor
                    .send(ClientRequest::PickWindow(atx))
                    .context("IPC communication channel closed")?;
                let result = arx
                    .recv()
                    .await
                    .context("Failed to receive picked window")?;
                Response::PickedWindow(result)
            }
            fht_compositor_ipc::Request::PickLayerShell => {
                let (atx, arx) = async_channel::bounded(1);
                to_compositor
                    .send(ClientRequest::PickLayerShell(atx))
                    .context("IPC communication channel closed")?;
                let result = arx
                    .recv()
                    .await
                    .context("Failed to receive picked layer-shell")?;
                Response::PickedLayerShell(result)
            }
            fht_compositor_ipc::Request::Action(action) => {
                let (atx, arx) = async_channel::bounded(1);
                to_compositor
                    .send(ClientRequest::Action(action, atx))
                    .context("IPC communication channel closed")?;
                let result = arx
                    .recv()
                    .await
                    .context("Failed to receive action result")?;
                match result {
                    Ok(()) => Response::Noop,
                    Err(err) => Response::Error(err.to_string()),
                }
            }
        };

        tx.send(res).await?;
    }

    Ok(())
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
            ClientRequest::Windows(tx) => {
                let focus = self.fht.keyboard.current_focus();
                let windows = self
                    .fht
                    .space
                    .monitors()
                    .flat_map(|mon| {
                        mon.workspaces()
                            .flat_map(|ws| workspace_windows(ws, focus.as_ref()))
                    })
                    .collect();

                tx.send_blocking(windows)?;
            }
            ClientRequest::LayerShells(tx) => {
                let mut layers = Vec::new();
                for output in self.fht.space.outputs() {
                    let layer_map = layer_map_for_output(output);
                    let output_name = output.name();
                    for layer_surface in layer_map.layers() {
                        layers.push(fht_compositor_ipc::LayerShell {
                            namespace: layer_surface.namespace().to_string(),
                            output: output_name.clone(),
                            // SAFETY: We know that all the enum variants are the same
                            #[allow(clippy::missing_transmute_annotations)]
                            layer: unsafe { std::mem::transmute(layer_surface.layer()) },
                            #[allow(clippy::missing_transmute_annotations)]
                            keyboard_interactivity: unsafe {
                                std::mem::transmute(
                                    layer_surface.cached_state().keyboard_interactivity,
                                )
                            },
                        })
                    }
                }

                tx.send_blocking(layers)?;
            }
            ClientRequest::Space(tx) => {
                let monitors = self
                    .fht
                    .space
                    .monitors()
                    .map(|mon| {
                        let output = mon.output().name();
                        let workspaces = mon
                            .workspaces()
                            .map(|workspace| {
                                let workspace_id = *workspace.id();

                                fht_compositor_ipc::Workspace {
                                    output: mon.output().name(),
                                    id: workspace_id,
                                    active_window_idx: workspace.active_tile_idx(),
                                    fullscreen_window_idx: workspace.fullscreened_tile_idx(),
                                    mwfact: workspace.mwfact(),
                                    nmaster: workspace.nmaster(),
                                    windows: workspace
                                        .windows()
                                        .map(Window::id)
                                        .map(|id| *id)
                                        .collect(),
                                }
                            })
                            .collect::<Vec<_>>()
                            .try_into()
                            .expect("workspace number is always 9");

                        (
                            output.clone(),
                            fht_compositor_ipc::Monitor {
                                output,
                                workspaces,
                                active: mon.active(),
                                active_workspace_idx: mon.active_workspace_idx(),
                            },
                        )
                    })
                    .collect();

                tx.send_blocking(fht_compositor_ipc::Space {
                    monitors,
                    active_idx: self.fht.space.active_monitor_idx(),
                    primary_idx: self.fht.space.primary_monitor_idx(),
                })?;
            }
            ClientRequest::Window { id, sender } => {
                let res = match id {
                    Some(id) => self.fht.space.monitors().find_map(|mon| {
                        mon.workspaces().find_map(|ws| {
                            ws.tiles()
                                .find(|tile| tile.window().id() == id)
                                .map(|tile| (tile, ws))
                        })
                    }),
                    None => {
                        let monitor = self.fht.space.active_monitor();
                        let workspace = monitor.active_workspace();
                        workspace.active_tile().map(|tile| (tile, workspace))
                    }
                };

                let window = res.map(|(tile, workspace)| {
                    let window = tile.window();
                    let location = tile.location() + tile.window_loc();
                    let size = window.size();

                    fht_compositor_ipc::Window {
                        id: *window.id(),
                        title: window.title(),
                        app_id: window.app_id(),
                        output: workspace.output().name(),
                        workspace_idx: workspace.index(),
                        workspace_id: *workspace.id(),
                        size: (size.w as u32, size.h as u32),
                        location: location.into(),
                        fullscreened: window.fullscreen(),
                        maximized: window.maximized(),
                        tiled: window.tiled(),
                        // NOTE: We can hardcode these two
                        activated: true,
                        focused: true,
                    }
                });

                sender.send_blocking(window)?;
            }
            ClientRequest::Workspace { id, sender } => {
                let workspace = match id {
                    Some(id) => self.fht.space.workspace_for_id(WorkspaceId(id)),
                    None => Some(self.fht.space.active_workspace()),
                };

                let workspace = workspace.map(|workspace| fht_compositor_ipc::Workspace {
                    output: workspace.output().name(),
                    id: *workspace.id(),
                    active_window_idx: workspace.active_tile_idx(),
                    fullscreen_window_idx: workspace.fullscreened_tile_idx(),
                    mwfact: workspace.mwfact(),
                    nmaster: workspace.nmaster(),
                    windows: workspace.windows().map(Window::id).map(|id| *id).collect(),
                });

                sender.send_blocking(workspace)?;
            }
            ClientRequest::WorkspaceByIndex {
                output,
                index,
                sender,
            } => {
                let monitor = match output {
                    Some(name) => {
                        let Some(mon) = self
                            .fht
                            .space
                            .monitors()
                            .find(|mon| mon.output().name() == name)
                        else {
                            sender.send_blocking(None)?;
                            return Ok(());
                        };

                        mon
                    }
                    None => self.fht.space.active_monitor(),
                };

                // NOTE: For now we know that workspaces are static, but I do want to implement
                // a way for the user to set a fixed number of workspaces. (perhaps "dynamic" ones)
                // See #54
                if index > monitor.workspaces.len() {
                    sender.send_blocking(None)?;
                    return Ok(());
                }

                let workspace = &monitor.workspaces[index];
                let ipc_workspace = fht_compositor_ipc::Workspace {
                    output: workspace.output().name(),
                    id: *workspace.id(),
                    active_window_idx: workspace.active_tile_idx(),
                    fullscreen_window_idx: workspace.fullscreened_tile_idx(),
                    mwfact: workspace.mwfact(),
                    nmaster: workspace.nmaster(),
                    windows: workspace.windows().map(Window::id).map(|id| *id).collect(),
                };

                sender.send_blocking(Some(ipc_workspace))?;
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
            ClientRequest::Action(action, tx) => {
                tx.send_blocking(self.handle_ipc_action(action))?;
            }
            // Subscribe workspace
            ClientRequest::SubscribeWorkspace { id, sender } => {
                let workspace = match id {
                    Some(id) => self.fht.space.workspace_for_id(WorkspaceId(id)),
                    None => Some(self.fht.space.active_workspace()),
                };

                let workspace = workspace.map(|workspace| fht_compositor_ipc::Workspace {
                    output: workspace.output().name(),
                    id: *workspace.id(),
                    active_window_idx: workspace.active_tile_idx(),
                    fullscreen_window_idx: workspace.fullscreened_tile_idx(),
                    mwfact: workspace.mwfact(),
                    nmaster: workspace.nmaster(),
                    windows: workspace.windows().map(Window::id).map(|id| *id).collect(),
                });

                let _ = sender.send_blocking(workspace);
                if let Ok(mut ipc) = crate::ipc::IPC_SUB_STATE.lock() {
                    ipc.subscribers_workspace
                        .entry(id.unwrap_or(usize::MAX))
                        .or_default()
                        .push(sender);
                }
            }

            // Subscribe windows
            ClientRequest::SubscribeWindows(tx) => {
                let focus = self.fht.keyboard.current_focus();
                let windows = self
                    .fht
                    .space
                    .monitors()
                    .flat_map(|mon| {
                        mon.workspaces()
                            .flat_map(|ws| workspace_windows(ws, focus.as_ref()))
                    })
                    .collect();

                let _ = tx.send_blocking(windows);
                if let Ok(mut ipc) = crate::ipc::IPC_SUB_STATE.lock() {
                    ipc.subscribers_windows.push(tx);
                }
            }

            // Subscribe space
            ClientRequest::SubscribeSpace(tx) => {
                let monitors = self
                    .fht
                    .space
                    .monitors()
                    .map(|mon| {
                        let output = mon.output().name();
                        let workspaces = mon
                            .workspaces()
                            .map(|workspace| {
                                let workspace_id = *workspace.id();

                                fht_compositor_ipc::Workspace {
                                    output: mon.output().name(),
                                    id: workspace_id,
                                    active_window_idx: workspace.active_tile_idx(),
                                    fullscreen_window_idx: workspace.fullscreened_tile_idx(),
                                    mwfact: workspace.mwfact(),
                                    nmaster: workspace.nmaster(),
                                    windows: workspace
                                        .windows()
                                        .map(Window::id)
                                        .map(|id| *id)
                                        .collect(),
                                }
                            })
                            .collect::<Vec<_>>()
                            .try_into()
                            .expect("workspace number is always 9");

                        (
                            output.clone(),
                            fht_compositor_ipc::Monitor {
                                output,
                                workspaces,
                                active: mon.active(),
                                active_workspace_idx: mon.active_workspace_idx(),
                            },
                        )
                    })
                    .collect();

                let _ = tx.send_blocking(fht_compositor_ipc::Space {
                    monitors,
                    active_idx: self.fht.space.active_monitor_idx(),
                    primary_idx: self.fht.space.primary_monitor_idx(),
                });

                if let Ok(mut ipc) = crate::ipc::IPC_SUB_STATE.lock() {
                    ipc.subscribers_space.push(tx);
                }
            }

            // Subscribe layer shells
            ClientRequest::SubscribeLayerShells(tx) => {
                let mut layers = Vec::new();
                for output in self.fht.space.outputs() {
                    let layer_map = layer_map_for_output(output);
                    let output_name = output.name();
                    for layer_surface in layer_map.layers() {
                        layers.push(fht_compositor_ipc::LayerShell {
                            namespace: layer_surface.namespace().to_string(),
                            output: output_name.clone(),
                            // COPYPASED SAFETY: We know that all the enum variants are the same
                            #[allow(clippy::missing_transmute_annotations)]
                            layer: unsafe { std::mem::transmute(layer_surface.layer()) },
                            #[allow(clippy::missing_transmute_annotations)]
                            keyboard_interactivity: unsafe {
                                std::mem::transmute(
                                    layer_surface.cached_state().keyboard_interactivity,
                                )
                            },
                        })
                    }
                }

                let _ = tx.send_blocking(layers);
                if let Ok(mut ipc) = crate::ipc::IPC_SUB_STATE.lock() {
                    ipc.subscribers_layer_shells.push(tx);
                }
            }

            // Subscribe single window
            ClientRequest::SubscribeWindow { id, sender } => {
                let res = match id {
                    Some(id) => self.fht.space.monitors().find_map(|mon| {
                        mon.workspaces().find_map(|ws| {
                            ws.tiles()
                                .find(|tile| tile.window().id() == id)
                                .map(|tile| (tile, ws))
                        })
                    }),
                    None => {
                        let monitor = self.fht.space.active_monitor();
                        let workspace = monitor.active_workspace();
                        workspace.active_tile().map(|tile| (tile, workspace))
                    }
                };

                let window = res.map(|(tile, workspace)| {
                    let window = tile.window();
                    let location = tile.location() + tile.window_loc();
                    let size = window.size();

                    fht_compositor_ipc::Window {
                        id: *window.id(),
                        title: window.title(),
                        app_id: window.app_id(),
                        output: workspace.output().name(),
                        workspace_idx: workspace.index(),
                        workspace_id: *workspace.id(),
                        size: (size.w as u32, size.h as u32),
                        location: location.into(),
                        fullscreened: window.fullscreen(),
                        maximized: window.maximized(),
                        tiled: window.tiled(),
                        // NOTE: We can hardcode these two
                        activated: true,
                        focused: true,
                    }
                });

                let _ = sender.send_blocking(window);
                if let Ok(mut ipc) = crate::ipc::IPC_SUB_STATE.lock() {
                    ipc.subscribers_window
                        .entry(id.unwrap_or(usize::MAX))
                        .or_default()
                        .push(sender);
                }
            }
        }

        Ok(())
    }

    fn handle_ipc_action(&mut self, action: fht_compositor_ipc::Action) -> anyhow::Result<()> {
        match action {
            fht_compositor_ipc::Action::Quit => self.fht.stop = true,
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
                    // FIXME: Figure out whether we should error or actually tell the user about
                    // the fact the window is not floating? Key-actions just ignore silently
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
                    // FIXME: Figure out whether we should error or actually tell the user about
                    // the fact the window is not floating? Key-actions just ignore silently
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

fn workspace_windows(
    workspace: &Workspace,
    keyboard_focus: Option<&KeyboardFocusTarget>,
) -> HashMap<usize, fht_compositor_ipc::Window> {
    let mut windows = HashMap::with_capacity(workspace.windows().len());
    let is_focused = move |window| matches!(&keyboard_focus, Some(KeyboardFocusTarget::Window(w)) if w == window);
    let output = workspace.output().name();
    let workspace_id = *workspace.id();
    let active_tile_idx = workspace.active_tile_idx();

    for (tile_idx, tile) in workspace.tiles().enumerate() {
        let window = tile.window();
        let location = tile.location() + tile.window_loc();
        let size = window.size();

        windows.insert(
            *window.id(),
            fht_compositor_ipc::Window {
                id: *window.id(),
                title: window.title(),
                app_id: window.app_id(),
                output: output.clone(),
                workspace_idx: workspace.index(),
                workspace_id,
                size: (size.w as u32, size.h as u32),
                location: location.into(),
                fullscreened: window.fullscreen(),
                maximized: window.maximized(),
                tiled: window.tiled(),
                activated: Some(tile_idx) == active_tile_idx,
                focused: is_focused(window),
            },
        );
    }

    windows
}
