//! An IPC based on D-bus.

mod output;
mod workspace;

pub use output::{Output as IpcOutput, Request as IpcOutputRequest};
use smithay::reexports::calloop::{self, LoopHandle};
use smithay::wayland::shell::xdg::XdgShellHandler;
pub use workspace::{Request as IpcWorkspaceRequest, Workspace as IpcWorkspace};
use zbus::{interface, zvariant};

use crate::config::CONFIG;
use crate::shell::workspaces::tile::WorkspaceElement;
use crate::state::State;
use crate::utils::dbus::DBUS_CONNECTION;
use crate::utils::geometry::RectCenterExt;
use crate::utils::output::OutputExt;

pub struct Ipc {
    /// Sender to the compositor state for it process the request.
    to_compositor: calloop::channel::Sender<IpcRequest>,
    /// Receiver from the compositor to get back the response.
    from_compositor: async_std::channel::Receiver<IpcResponse>,
}

pub enum IpcRequest {
    /// Reload the configuration.
    ReloadConfig,

    /// Get a list of all the registered outputs object paths'
    ListOutputs,

    /// Get the title of the window with this protocol ID.
    GetWindowTitle { window_id: u64 },

    /// Get the workspace path holding the window with this protocol ID.
    GetWindowWorkspace { window_id: u64 },

    /// Get the app_id/WM_CLASS of the window with this protocol ID.
    GetWindowAppId { window_id: u64 },

    /// Get the tiled state of the window with this protocol ID.
    GetWindowTiled { window_id: u64 },

    /// Set the tiled state of the window with this protocol ID.
    SetWindowTiled { window_id: u64, tiled: bool },

    /// Get the fullscreen state of the window with this protocol ID.
    GetWindowFullscreened { window_id: u64 },

    /// Set the fullscreen state of the window with this protocol ID.
    SetWindowFullscreened { window_id: u64, fullscreened: bool },

    /// Get the maximized state of the window with this protocol ID.
    GetWindowMaximized { window_id: u64 },

    /// Set the maximized state of the window with this protocol ID.
    SetWindowMaximized { window_id: u64, maximized: bool },

    /// Set The active output.
    SetFocusedOutput { name: String },
}

pub enum IpcResponse {
    // Reponses for requests.
    InvalidProtocolId,
    WindowPropString(String),
    WindowPropBool(bool),
    Outputs(Vec<String>),
}

#[interface(name = "fht.desktop.Compositor.Ipc")]
impl Ipc {
    async fn reload_config(&self) {
        if let Err(err) = self.to_compositor.send(IpcRequest::ReloadConfig) {
            warn!(?err, "Failed to send IPC request to the compositor!");
        }
    }

    async fn list_outputs(&self) -> zbus::fdo::Result<Vec<zvariant::ObjectPath>> {
        if let Err(err) = self.to_compositor.send(IpcRequest::ListOutputs) {
            warn!(?err, "Failed to send IPC request to the compositor!");
            return Err(zbus::fdo::Error::Failed(
                "Failed to send request to the compositor!".to_string(),
            ));
        }

        match self.from_compositor.recv().await {
            Ok(IpcResponse::Outputs(outputs)) => Ok(outputs
                .into_iter()
                .filter_map(|path| zvariant::ObjectPath::try_from(path).ok())
                .collect()),
            Ok(_) => panic!("Something went really wrong..."),
            Err(err) => Err(zbus::fdo::Error::Failed(err.to_string())),
        }
    }

    async fn get_window_title(&self, window_id: u64) -> zbus::fdo::Result<String> {
        if let Err(err) = self
            .to_compositor
            .send(IpcRequest::GetWindowTitle { window_id })
        {
            warn!(?err, "Failed to send IPC request to the compositor");
            return Err(zbus::fdo::Error::Failed(
                "Failed to send request to the compositor!".to_string(),
            ));
        };

        match self.from_compositor.recv().await {
            Ok(IpcResponse::WindowPropString(title)) => Ok(title),
            Ok(_) => panic!("Something went really wrong..."),
            Err(err) => Err(zbus::fdo::Error::Failed(err.to_string())),
        }
    }

    async fn get_window_workspace(
        &self,
        window_id: u64,
    ) -> zbus::fdo::Result<zvariant::ObjectPath> {
        if let Err(err) = self
            .to_compositor
            .send(IpcRequest::GetWindowWorkspace { window_id })
        {
            warn!(?err, "Failed to send IPC request to the compositor");
            return Err(zbus::fdo::Error::Failed(
                "Failed to send request to the compositor!".to_string(),
            ));
        };

        match self.from_compositor.recv().await {
            // SAFETY: The path should be checked beforehand
            Ok(IpcResponse::WindowPropString(path)) => Ok(path.try_into().unwrap()),
            Ok(_) => panic!("Something went really wrong..."),
            Err(err) => Err(zbus::fdo::Error::Failed(err.to_string())),
        }
    }

    async fn get_window_app_id(&self, window_id: u64) -> zbus::fdo::Result<String> {
        if let Err(err) = self
            .to_compositor
            .send(IpcRequest::GetWindowAppId { window_id })
        {
            warn!(?err, "Failed to send IPC request to the compositor");
            return Err(zbus::fdo::Error::Failed(
                "Failed to send request to the compositor!".to_string(),
            ));
        };

        match self.from_compositor.recv().await {
            Ok(IpcResponse::WindowPropString(title)) => Ok(title),
            Ok(_) => panic!("Something went really wrong..."),
            Err(err) => Err(zbus::fdo::Error::Failed(err.to_string())),
        }
    }

    async fn get_window_tiled(&self, window_id: u64) -> zbus::fdo::Result<bool> {
        if let Err(err) = self
            .to_compositor
            .send(IpcRequest::GetWindowTiled { window_id })
        {
            warn!(?err, "Failed to send IPC request to the compositor");
            return Err(zbus::fdo::Error::Failed(
                "Failed to send request to the compositor!".to_string(),
            ));
        };

        match self.from_compositor.recv().await {
            Ok(IpcResponse::WindowPropBool(tiled)) => Ok(tiled),
            Ok(_) => panic!("Something went really wrong..."),
            Err(err) => Err(zbus::fdo::Error::Failed(err.to_string())),
        }
    }

    async fn set_window_tiled(&self, window_id: u64, tiled: bool) -> zbus::fdo::Result<()> {
        if let Err(err) = self
            .to_compositor
            .send(IpcRequest::SetWindowTiled { window_id, tiled })
        {
            warn!(?err, "Failed to send IPC request to the compositor");
            return Err(zbus::fdo::Error::Failed(
                "Failed to send request to the compositor!".to_string(),
            ));
        } else {
            Ok(())
        }
    }

    async fn get_window_fullscreened(&self, window_id: u64) -> zbus::fdo::Result<bool> {
        if let Err(err) = self
            .to_compositor
            .send(IpcRequest::GetWindowFullscreened { window_id })
        {
            warn!(?err, "Failed to send IPC request to the compositor");
            return Err(zbus::fdo::Error::Failed(
                "Failed to send request to the compositor!".to_string(),
            ));
        };

        match self.from_compositor.recv().await {
            Ok(IpcResponse::WindowPropBool(fullscreened)) => Ok(fullscreened),
            Ok(_) => panic!("Something went really wrong..."),
            Err(err) => Err(zbus::fdo::Error::Failed(err.to_string())),
        }
    }

    async fn set_window_fullscreened(
        &self,
        window_id: u64,
        fullscreened: bool,
    ) -> zbus::fdo::Result<()> {
        if let Err(err) = self.to_compositor.send(IpcRequest::SetWindowFullscreened {
            window_id,
            fullscreened,
        }) {
            warn!(?err, "Failed to send IPC request to the compositor");
            return Err(zbus::fdo::Error::Failed(
                "Failed to send request to the compositor!".to_string(),
            ));
        } else {
            Ok(())
        }
    }

    async fn get_window_maximized(&self, window_id: u64) -> zbus::fdo::Result<bool> {
        if let Err(err) = self
            .to_compositor
            .send(IpcRequest::GetWindowMaximized { window_id })
        {
            warn!(?err, "Failed to send IPC request to the compositor");
            return Err(zbus::fdo::Error::Failed(
                "Failed to send request to the compositor!".to_string(),
            ));
        };

        match self.from_compositor.recv().await {
            Ok(IpcResponse::WindowPropBool(maximized)) => Ok(maximized),
            Ok(_) => panic!("Something went really wrong..."),
            Err(err) => Err(zbus::fdo::Error::Failed(err.to_string())),
        }
    }

    async fn set_window_maximized(&self, window_id: u64, maximized: bool) -> zbus::fdo::Result<()> {
        if let Err(err) = self.to_compositor.send(IpcRequest::SetWindowMaximized {
            window_id,
            maximized,
        }) {
            warn!(?err, "Failed to send IPC request to the compositor");
            return Err(zbus::fdo::Error::Failed(
                "Failed to send request to the compositor!".to_string(),
            ));
        } else {
            Ok(())
        }
    }

    async fn set_focused_output(&self, name: String) -> zbus::fdo::Result<()> {
        if let Err(err) = self
            .to_compositor
            .send(IpcRequest::SetFocusedOutput { name })
        {
            warn!(?err, "Failed to send IPC request to the compositor");
            return Err(zbus::fdo::Error::Failed(
                "Failed to send request to the compositor!".to_string(),
            ));
        } else {
            Ok(())
        }
    }
}

/// Start the fht-compositor IPC server on the session D-bus.
///
/// This will register the ervice `fht.desktop.Compositor` with the interface
/// `fht.desktop.Compositor.Ipc` to interface with it.
pub fn start(loop_handle: &LoopHandle<'static, State>) -> zbus::Result<()> {
    // In order to communicate with the compositor, we need two channels.
    //
    // - going from the IPC to the compositor so we can process the request. This is going to be a
    //   calloop channel since we don't have any access to the [`State`] here.
    let (to_compositor, from_ipc_channel) = calloop::channel::channel::<IpcRequest>();
    // - going from the compositor to the IPC, so we can send the response back to the dbus client
    //   calling us. Same reason apply here (no access to the [`State`]), + dbus should be
    //   asynchronous, compared to wayland synchronous system.
    let (to_ipc, from_compositor) = async_std::channel::unbounded::<IpcResponse>();

    // Now create the dbus connection.
    //
    // We always serve `CompositorIpc`, but additional paths/interfaces are created while the
    // compositor runs.
    DBUS_CONNECTION
        .object_server()
        .at(
            "/fht/desktop/Compositor",
            Ipc {
                from_compositor,
                to_compositor,
            },
        )
        .expect("Failed to expose main IPC interface!");

    loop_handle
        .insert_source(from_ipc_channel, move |request, _, state| {
            let calloop::channel::Event::Msg(req) = request else {
                return;
            };
            state.handle_ipc_request(req, &to_ipc);
        })
        .expect("Failed to insert IPC event source!");

    Ok(())
}

impl State {
    /// Process a given IPC request.
    #[profiling::function]
    fn handle_ipc_request(
        &mut self,
        req: IpcRequest,
        to_ipc: &async_std::channel::Sender<IpcResponse>,
    ) {
        match req {
            IpcRequest::ReloadConfig => self.reload_config(),
            IpcRequest::ListOutputs => {
                let ret = self
                    .fht
                    .outputs()
                    .map(|o| {
                        format!(
                            "/fht/desktop/Compositor/Output/{}",
                            o.name().replace("-", "_")
                        )
                    })
                    .collect();

                to_ipc.send_blocking(IpcResponse::Outputs(ret)).unwrap();
            }
            IpcRequest::GetWindowTitle { window_id } => {
                if let Some(window) = self
                    .fht
                    .all_windows()
                    .find(|window| window.uid() == window_id)
                {
                    to_ipc
                        .send_blocking(IpcResponse::WindowPropString(window.title()))
                        .unwrap();
                } else {
                    to_ipc
                        .send_blocking(IpcResponse::InvalidProtocolId)
                        .unwrap();
                }
            }
            IpcRequest::GetWindowWorkspace { window_id } => {
                if let Some(window) = self
                    .fht
                    .all_windows()
                    .find(|window| window.uid() == window_id)
                {
                    let workspace = self.fht.ws_for(window).unwrap();
                    let ipc_path = workspace.ipc_path.clone().as_ref().to_string();
                    to_ipc
                        .send_blocking(IpcResponse::WindowPropString(ipc_path))
                        .unwrap();
                } else {
                    to_ipc
                        .send_blocking(IpcResponse::InvalidProtocolId)
                        .unwrap();
                }
            }
            IpcRequest::GetWindowAppId { window_id } => {
                if let Some(window) = self
                    .fht
                    .all_windows()
                    .find(|window| window.uid() == window_id)
                {
                    to_ipc
                        .send_blocking(IpcResponse::WindowPropString(window.app_id()))
                        .unwrap();
                } else {
                    to_ipc
                        .send_blocking(IpcResponse::InvalidProtocolId)
                        .unwrap();
                }
            }
            IpcRequest::GetWindowTiled { window_id } => {
                if let Some(window) = self
                    .fht
                    .all_windows()
                    .find(|window| window.uid() == window_id)
                {
                    to_ipc
                        .send_blocking(IpcResponse::WindowPropBool(true))
                        .unwrap();
                } else {
                    to_ipc
                        .send_blocking(IpcResponse::InvalidProtocolId)
                        .unwrap();
                }
            }
            IpcRequest::SetWindowTiled { window_id, tiled } => {
                if let Some(window) = self
                    .fht
                    .all_windows()
                    .find(|window| window.uid() == window_id)
                {
                    // window.set_tiled(tiled);
                    // window.toplevel().send_pending_configure();
                    // self.fht.ws_for(window).unwrap().refresh_window_geometries();
                }
            }
            IpcRequest::GetWindowMaximized { window_id } => {
                if let Some(window) = self
                    .fht
                    .all_windows()
                    .find(|window| window.uid() == window_id)
                {
                    to_ipc
                        .send_blocking(IpcResponse::WindowPropBool(window.maximized()))
                        .unwrap();
                } else {
                    to_ipc
                        .send_blocking(IpcResponse::InvalidProtocolId)
                        .unwrap();
                }
            }
            IpcRequest::SetWindowMaximized {
                window_id,
                maximized,
            } => {
                if let Some(window) = self
                    .fht
                    .all_windows()
                    .find(|window| window.uid() == window_id)
                {
                    window.set_maximized(maximized);
                    window.toplevel().unwrap().send_pending_configure();
                    self.fht.ws_for(window).unwrap().refresh_window_geometries();
                }
            }
            IpcRequest::GetWindowFullscreened { window_id } => {
                if let Some(window) = self
                    .fht
                    .all_windows()
                    .find(|window| window.uid() == window_id)
                {
                    to_ipc
                        .send_blocking(IpcResponse::WindowPropBool(window.fullscreen()))
                        .unwrap();
                } else {
                    to_ipc
                        .send_blocking(IpcResponse::InvalidProtocolId)
                        .unwrap();
                }
            }
            IpcRequest::SetWindowFullscreened {
                window_id,
                fullscreened,
            } => {
                let maybe_window = self
                    .fht
                    .all_windows()
                    .find(|window| window.uid() == window_id)
                    .cloned();
                if let Some(window) = maybe_window {
                    if fullscreened {
                        let toplevel = window.toplevel().unwrap().clone();
                        self.fullscreen_request(toplevel, None);
                    } else {
                        window.set_fullscreen(false);
                        window.set_fullscreen_output(None);
                        let workspace = self.fht.ws_mut_for(&window).unwrap();
                        workspace.remove_current_fullscreen();
                    }
                }
            }
            IpcRequest::SetFocusedOutput { name } => {
                if let Some(output) = self.fht.output_named(&name) {
                    if CONFIG.general.cursor_warps {
                        let center = output.geometry().center();
                        self.move_pointer(center.to_f64());
                    }
                    self.fht.focus_state.output = Some(output);
                }
            }
        }
    }
}
