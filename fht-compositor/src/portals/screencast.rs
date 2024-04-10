use std::collections::HashMap;

use smithay::reexports::calloop;
use smithay::utils::Rectangle;
use zbus::object_server::SignalContext;
use zbus::{interface, ObjectServer};

use crate::backend::Backend;
use crate::state::{Fht, State};
use crate::utils::dbus::DBUS_CONNECTION;
use crate::utils::geometry::Global;
use crate::utils::output::OutputExt;
use crate::utils::pipewire::PipeWire;

pub const PORTAL_VERSION: u32 = 5;

bitflags::bitflags! {
    /// org.freedesktop.impl.portal.ScreenCast:AvailableSourceTypes
    ///
    /// A bitmask of available source types. Currently defined types are:
    #[derive(Clone, Copy, PartialEq)]
    pub struct SourceType: u32 {
        const MONITOR = 1;
        const WINDOW = 2;
        const VIRTUAL = 4;
    }
}

bitflags::bitflags! {
    /// org.freedesktop.impl.portal.ScreenCast:AvailableCursorModes
    #[derive(Clone, Copy)]
    pub struct CursorMode: u32 {
        /// The cursor is not part of the screen cast stream.
        const HIDDEN = 1;
        /// The cursor is embedded as part of the stream buffers.
        const EMBEDDED = 2;
        /// The cursor is not part of the screen cast stream, but sent as PipeWire stream metadata.
        const METADATA = 4;
    }
}

pub struct Portal {
    /// Sender to the compositor state for it process the request.
    pub(super) to_compositor: calloop::channel::Sender<Request>,
    /// Receiver from the compositor to get back the response.
    pub(super) from_compositor: async_std::channel::Receiver<Response>,
}

pub enum Request {
    StartCast {
        session_handle: zvariant::OwnedObjectPath,
        source: SessionSource,
        source_type: SourceType,
        cursor_mode: CursorMode,
    },
    StopCast {
        session_handle: zvariant::OwnedObjectPath,
    },
}

pub enum Response {
    PipeWireStreamData {
        node_id: u32,
        location: (i32, i32),
        size: (i32, i32),
        source_type: u32,
    },
    PipeWireFail,
}

#[interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl Portal {
    #[zbus(property)]
    pub fn available_source_types(&self) -> u32 {
        SourceType::MONITOR.bits()
    }

    #[zbus(property)]
    pub fn available_cursor_modes(&self) -> u32 {
        (CursorMode::HIDDEN | CursorMode::EMBEDDED).bits()
    }

    #[zbus(property)]
    pub fn version(&self) -> u32 {
        PORTAL_VERSION
    }

    async fn create_session(
        &self,
        request_handle: zvariant::ObjectPath<'_>,
        session_handle: zvariant::ObjectPath<'_>,
        app_id: String,
        _options: HashMap<&str, zvariant::Value<'_>>,
        #[zbus(object_server)] object_server: &ObjectServer,
    ) -> (u32, HashMap<&str, zvariant::Value<'_>>) {
        debug!(
            request_handle = request_handle.to_string(),
            session_handle = session_handle.to_string(),
            ?app_id,
            "Creating screencast session."
        );

        // Setup request and session
        let request = PRequest {
            handle: request_handle.clone().into(),
        };
        if let Err(err) = object_server.at(&request_handle, request).await {
            warn!(
                ?err,
                session_handle = session_handle.to_string(),
                request_handle = request_handle.to_string(),
                "Failed to create screencast request handle!"
            );
            return (1, HashMap::new());
        };
        let session = Session {
            _app_id: app_id,
            request_handle: request_handle.clone().into(),
            handle: session_handle.clone().into(),
            cursor_mode: CursorMode::HIDDEN,
            source_type: SourceType::empty(),
            source: SessionSource::Unset,
        };
        if let Err(err) = object_server.at(&session_handle, session).await {
            let request_ref = object_server
                .interface::<_, PRequest>(&request_handle)
                .await
                .unwrap();
            let request = request_ref.get_mut().await;
            request.close(object_server).await;

            warn!(
                ?err,
                session_handle = session_handle.to_string(),
                "Failed to create screencast session handle!"
            );
            return (1, HashMap::new());
        };

        (0, HashMap::new())
    }

    async fn select_sources(
        &self,
        _request_handle: zvariant::ObjectPath<'_>,
        session_handle: zvariant::ObjectPath<'_>,
        _app_id: String,
        options: HashMap<&str, zvariant::Value<'_>>,
        #[zbus(object_server)] object_server: &ObjectServer,
    ) -> (u32, HashMap<&str, zvariant::Value<'_>>) {
        let session_ref = object_server
            .interface::<_, Session>(&session_handle)
            .await
            .unwrap();
        let mut session = session_ref.get_mut().await;

        let cursor_mode =
            CursorMode::from_bits(u32::try_from(options.get("cursor_mode").unwrap()).unwrap())
                .unwrap();

        let source_type =
            SourceType::from_bits(u32::try_from(options.get("types").unwrap()).unwrap()).unwrap();
        // TODO: Support multiple sources
        // TODO: Handle persist_mode
        let arg = match source_type {
            SourceType::MONITOR => "select_outputs",
            SourceType::VIRTUAL | SourceType::WINDOW => "select_area",
            value if value == SourceType::MONITOR | SourceType::VIRTUAL => "",
            _ => unreachable!(),
        };
        let output = async_std::process::Command::new("fht-share-picker")
            .stdout(std::process::Stdio::null())
            .arg(arg)
            .output()
            .await
            .expect("Failed to spawn command!");
        if !output.status.success() {
            warn!(
                session_handle = session_handle.to_string(),
                "Share picker exited unsuccessfully"
            );
            session.close(object_server).await;
            return (1, HashMap::new());
        }
        let stderr = std::str::from_utf8(&output.stderr).expect("stderr contained invalid bytes!");
        let source = if let Some((x, y, w, h)) = stderr
            .lines()
            .find(|line| line.contains("[select-area]"))
            .and_then(|line| {
                let coords_str = line.split('/').skip(1).next()?;
                let coords: (i32, i32, i32, i32) = ron::de::from_str(coords_str).ok()?;
                Some(coords)
            }) {
            SessionSource::Rectangle(Rectangle::from_loc_and_size((x, y), (w, h)), None)
        } else if let Some(output_name) = stderr
            .lines()
            .find(|line| line.contains("[select-output]"))
            .and_then(|line| line.split('/').skip(1).next())
        {
            SessionSource::Output(output_name.to_string(), None)
        } else {
            warn!(
                session_handle = session_handle.to_string(),
                "Unable to select source for screencopy!"
            );
            session.close(object_server).await;
            return (1, HashMap::new());
        };

        session.source_type = source_type;
        session.source = source;
        session.cursor_mode = cursor_mode;

        (0, HashMap::new())
    }

    async fn start(
        &self,
        _request_handle: zvariant::ObjectPath<'_>,
        session_handle: zvariant::ObjectPath<'_>,
        _app_id: String,
        _parent_window: String,
        _options: HashMap<&str, zvariant::Value<'_>>,
        #[zbus(object_server)] object_server: &ObjectServer,
    ) -> (u32, HashMap<&str, zvariant::Value<'_>>) {
        // TODO: Support multiple sessions
        let session_ref = object_server
            .interface::<_, Session>(&session_handle)
            .await
            .unwrap();
        let session = session_ref.get_mut().await;

        if let Err(err) = self.to_compositor.send(Request::StartCast {
            session_handle: session_handle.clone().into(),
            source: session.source.clone(),
            source_type: session.source_type,
            cursor_mode: session.cursor_mode,
        }) {
            warn!(?err, "PipeWire failed to start cast!");
            session.close(object_server).await;
            return (1, HashMap::new());
        }

        let (node_id, location, size, source_type) = match self.from_compositor.recv().await {
            Ok(Response::PipeWireStreamData {
                node_id,
                location,
                size,
                source_type,
            }) => (node_id, location, size, source_type),
            Ok(Response::PipeWireFail) | Err(_) => {
                error!("Pipewire failed to start cast!");
                session.close(object_server).await;
                return (1, HashMap::new());
            }
        };

        let mut results = HashMap::new();

        results.insert(
            "streams",
            zvariant::Value::new(vec![(node_id, {
                let mut response = HashMap::new();
                response.insert("position", zvariant::Value::new(location));
                response.insert("size", zvariant::Value::new(size));
                response.insert("source_type", zvariant::Value::new(source_type));

                response
            })]),
        );

        //    node_id + data
        (0, results)
    }
}

#[derive(Clone, PartialEq)]
pub enum SessionSource {
    /// The session source is unset.
    ///
    /// This means we didn't call SelectSource for this session yet.
    Unset,
    /// A named output.
    Output(String, Option<smithay::output::Output>),
    /// An area in compositor space.
    Rectangle(Rectangle<i32, Global>, Option<smithay::output::Output>),
}

impl SessionSource {
    /// Get the output containing this source.
    pub fn output(&self) -> Option<&smithay::output::Output> {
        match self {
            Self::Unset => None,
            Self::Output(_, output) | Self::Rectangle(_, output) => output.as_ref(),
        }
    }

    /// Get the rectangle of this source.
    pub fn rectangle(&self) -> Option<Rectangle<i32, Global>> {
        match self {
            Self::Unset => None,
            Self::Output(_, output) => output.as_ref().map(|o| o.geometry()),
            Self::Rectangle(rec, _) => Some(*rec),
        }
    }
}

pub struct Session {
    _app_id: String,
    request_handle: zvariant::OwnedObjectPath,
    handle: zvariant::OwnedObjectPath,
    cursor_mode: CursorMode,
    // TODO: Multiple source support
    source_type: SourceType,
    source: SessionSource,
}

#[interface(name = "org.freedesktop.impl.portal.Session")]
impl Session {
    async fn close(&self, #[zbus(object_server)] object_server: &ObjectServer) {
        // We should have this object if we are being called.
        assert!(object_server
            .remove::<Session, _>(&self.handle)
            .await
            .unwrap());
        // And if we have a session we surely have the request too
        assert!(object_server
            .remove::<PRequest, _>(&self.request_handle)
            .await
            .unwrap());
    }

    #[zbus(signal)]
    async fn closed(&self, signal_ctx: &SignalContext<'_>) -> zbus::Result<()>;
}

/// Not to be confused with [`Request`]
///
/// This is the Portal Request that is used by the application requesting the screencast.
pub struct PRequest {
    handle: zvariant::OwnedObjectPath,
}

#[interface(name = "org.freedesktop.impl.Portal.Request")]
impl PRequest {
    async fn close(&self, #[zbus(object_server)] object_server: &ObjectServer) {
        assert!(object_server
            .remove::<PRequest, _>(&self.handle)
            .await
            .unwrap());
    }

    #[zbus(signal)]
    async fn closed(&self, signal_ctx: &SignalContext<'_>) -> zbus::Result<()>;
}

impl State {
    pub(super) fn handle_screencast_request(
        &mut self,
        req: Request,
        to_screencast: &async_std::channel::Sender<Response>,
        to_compositor: &calloop::channel::Sender<Request>,
    ) {
        match req {
            Request::StartCast {
                session_handle,
                mut source,
                source_type,
                .. // TODO: Take in account of cursor_mode
            } => {
                // We don't support screencasting on X11 since eh, you prob dont need it.
                #[cfg(feature = "udev_backend")]
                let Backend::Udev(ref mut data) = &mut self.backend
                else {
                    warn!("ScreenCast is only supported on udev backend!");
                    return;
                };
                #[cfg(not(feature = "udev_backend"))]
                {
                    warn!("ScreenCast is only supported on udev backend!");
                    return;
                }

                let Some(gbm_device) = data.devices.get(&data.primary_node).map(|d| d.gbm.clone())
                else {
                    warn!("No available GBM device!");
                    return;
                };

                match &mut source {
                    SessionSource::Unset => unreachable!(),
                    SessionSource::Output(name, output) => {
                        if output.is_none() {
                            info!(output_name = name);
                            if let Some(o) = self.fht.output_named(&name) {
                                *output = Some(o);
                            } else {
                                warn!("Tried to start a screencast with an invalid output!");
                                to_screencast.send_blocking(Response::PipeWireFail).unwrap();
                                    return;
                            }
                        }
                    }
                    SessionSource::Rectangle(rec, output) => {
                        if output.is_none() {
                            info!("Adding from rec");
                            if let Some(o) = self
                                .fht
                                .outputs()
                                .find(|o| o.geometry().intersection(*rec).is_some())
                                .cloned()
                            {
                                *output = Some(o);
                            } else {
                                warn!("Tried to start a screecast with an invalid region!");
                                to_screencast.send_blocking(Response::PipeWireFail).unwrap();
                                    return;
                            }
                        }
                    }
                }

                self.fht.pipewire_initialised.call_once(|| {
                    self.fht.pipewire = PipeWire::new(&self.fht.loop_handle)
                        .map_err(|err| {
                            warn!(
                                ?err,
                                "Failed to initialize PipeWire! ScreenCasts will NOT work!"
                            );
                        })
                        .ok();
                });

                let Some(pipewire) = self.fht.pipewire.as_mut() else {
                    warn!("PipeWire is not initialised!");
                    to_screencast.send_blocking(Response::PipeWireFail).unwrap();
                    return;
                };

                match pipewire.start_cast(
                    to_compositor.clone(),
                    to_screencast.clone(),
                    gbm_device,
                    session_handle,
                    source.clone(),
                    source_type,
                ) {
                    Ok(cast) => {
                        pipewire.casts.push(cast);
                    }
                    Err(err) => {
                        error!(?err, "Failed to start screen cast!");
                        to_screencast.send_blocking(Response::PipeWireFail).unwrap();
                    }
                }
            }
            _ => (),
        }
    }
}

impl Fht {
    #[profiling::function]
    pub fn stop_cast(&mut self, session_handle: zvariant::OwnedObjectPath) {
        let Some(pipewire) = self.pipewire.as_mut() else {
            return;
        };

        let Some(idx) = pipewire
            .casts
            .iter()
            .position(|c| c.session_handle == session_handle)
        else {
            warn!("Tried to stop an invalid cast!");
            return;
        };

        let cast = pipewire.casts.swap_remove(idx);
        if let Err(err) = cast.stream.disconnect() {
            warn!(?err, "Failed to disconnect PipeWire stream!");
        }

        let object_server = DBUS_CONNECTION.object_server();
        let Ok(interface) = object_server.interface::<_, Session>(&session_handle) else {
            warn!("Cast session doesn't exist!");
            return;
        };
        async_std::task::block_on(async {
            interface.get().close(object_server.inner()).await;
            if let Err(err) = interface.get().closed(interface.signal_context()).await {
                warn!(?err, "Failed to send closed signal to screencast session!");
            };
        });
        debug!(session_handle = session_handle.to_string(), "Stopped cast!");
    }
}
