use std::collections::HashMap;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};

use anyhow::Context;
use calloop::io::Async;
use fht_compositor_ipc::Response;
use futures_util::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{
    Dispatcher, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
};

use crate::output::OutputExt;
use crate::state::State;

/// The compositor IPC server.
pub struct Server {
    // The UnixSocket server that receives incoming clients
    listener_token: RegistrationToken,
    dispatcher: Dispatcher<'static, Generic<UnixListener>, State>,
    // The calloop channel sender to communicate from/to the compositor.
    to_compositor: calloop::channel::Sender<ClientRequest>,
}

impl Server {
    pub fn close(self, loop_handle: &LoopHandle<'static, State>) {
        loop_handle.remove(self.listener_token);
        let _listener = Dispatcher::into_source_inner(self.dispatcher).unwrap();
        // FIXME: Close socket?
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
    let socket_dir = xdg::BaseDirectories::new()
        .unwrap()
        .get_runtime_directory()
        .cloned()
        .unwrap();
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

                let to_compositor = to_compositor_.clone();
                let fut = async move {
                    if let Err(err) = handle_new_client(socket, to_compositor).await {
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
        to_compositor,
    })
}

async fn handle_new_client(
    stream: Async<'static, UnixStream>,
    to_compositor: calloop::channel::Sender<ClientRequest>,
) -> anyhow::Result<()> {
    crate::profile_function!();
    let (reader, mut writer) = stream.split();
    let mut reader = futures_util::io::BufReader::new(reader);

    // The IPC model requires each request to be on a single line.
    let mut req_buf = String::new();
    reader.read_line(&mut req_buf).await?;
    let request = serde_json::from_str::<fht_compositor_ipc::Request>(&req_buf);

    let response = match request {
        Ok(req) => handle_request(req, to_compositor)
            .await
            .map_err(|err| err.to_string()),
        Err(err) => Err(err.to_string()), // Just write an error string;
    };

    let mut response_str = serde_json::to_string(&response)?;
    response_str.push('\n'); // separate by newlines
    _ = writer.write(response_str.as_bytes()).await?;

    Ok(())
}

enum ClientRequest {
    Outputs(async_channel::Sender<HashMap<String, fht_compositor_ipc::Output>>),
}

async fn handle_request(
    req: fht_compositor_ipc::Request,
    to_compositor: calloop::channel::Sender<ClientRequest>,
) -> anyhow::Result<Response> {
    match req {
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
                            serial: output.serial(),
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
        }

        Ok(())
    }
}
