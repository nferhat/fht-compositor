//! XDG screencast implementation.
//!
//! This file only handles D-Bus communication. For pipewire logic, see `src/pipewire/mod.rs`
//!
//! A lot of architectural design from <https://github.com/waycrate/xdg-desktop-portal-luminous/>,
//! really good portal!

use std::collections::HashMap;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use smithay::reexports::calloop;
use smithay::utils::{Physical, Size};
use zbus::object_server::SignalEmitter;
use zbus::{fdo, interface, ObjectServer};
use zvariant::as_value::{self, optional};
use zvariant::{ObjectPath, OwnedValue};

use crate::utils::pipewire::CastId;

pub const PORTAL_VERSION: u32 = 5;

pub type ScreencastSession = super::shared::Session<SessionData>;
use super::shared::{PortalRequest, PortalResponse};

#[derive(zvariant::Type, Debug, Default, Serialize, Deserialize)]
/// Options dict specificed in a [`Portal::create_session`] request.
#[zvariant(signature = "dict")]
struct CreateSessionResult {
    #[serde(with = "as_value")]
    handle_token: String,
}

#[derive(zvariant::Type, Debug, Default, Serialize, Deserialize)]
/// Options dict specified in a [`Screencast::select_sources`] request.
#[zvariant(signature = "dict")]
pub struct SelectSourcesOptions {
    /// A string that will be used as the last element of the handle.
    /// What types of content to record.
    #[serde(with = "optional", skip_serializing_if = "Option::is_none", default)]
    pub types: Option<u32>,
    /// Whether to allow selecting multiple sources.
    #[serde(with = "optional", skip_serializing_if = "Option::is_none", default)]
    pub multiple: Option<bool>,
    /// Determines how the cursor will be drawn in the screen cast stream.
    #[serde(with = "optional", skip_serializing_if = "Option::is_none", default)]
    pub cursor_mode: Option<u32>,
    #[serde(with = "optional", skip_serializing_if = "Option::is_none", default)]
    pub restore_token: Option<String>,
    #[serde(with = "optional", skip_serializing_if = "Option::is_none", default)]
    pub persist_mode: Option<u32>,
}

#[derive(zvariant::Type, Debug, Default, Clone, Serialize, Deserialize)]
pub struct Stream(u32, StreamProperties);

#[derive(zvariant::Type, Debug, Default, Clone, Serialize, Deserialize)]
#[zvariant(signature = "dict")]
struct StreamProperties {
    #[serde(with = "as_value")]
    size: (i32, i32),
    #[serde(with = "as_value")]
    source_type: u32,
}

// TODO: this is copy from ashpd, but the dict is a little different from xdg_desktop_portal
#[derive(zvariant::Type, Debug, Default, Serialize, Deserialize)]
#[zvariant(signature = "dict")]
struct StartResult {
    #[serde(with = "as_value")]
    streams: Vec<Stream>,
    #[serde(with = "as_value")]
    persist_mode: u32,
    #[serde(with = "optional", skip_serializing_if = "Option::is_none", default)]
    restore_token: Option<String>,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct SourceType: u32 {
        const MONITOR = 1;
        const WINDOW = 2;
        const VIRTUAL = 4;
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, PartialEq)]
    pub struct CursorMode: u32 {
        const HIDDEN = 1;
        const EMBEDDED = 2;
        const METADATA = 4;
    }
}

/// A [XDG ScreenCast desktop portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html) instance
///
/// This structure can be added inside a zbus [`Connection`] to register the
/// `org.freedesktop.impl.portal.ScreenCast` interface
pub struct Portal {
    to_compositor: calloop::channel::Sender<Request>,
    span: tracing::Span,
}

impl Portal {
    pub fn new(to_compositor: calloop::channel::Sender<Request>) -> Self {
        Self {
            to_compositor,
            span: tracing::debug_span!("xdg-screencast-portal"),
        }
    }
}

/// A [`Request`] that the [`Portal`] or a [`Session`] can send to the compositor.
pub enum Request {
    /// The [`Portal`] has requested to start a cast.
    StartCast {
        session_handle: zvariant::OwnedObjectPath,
        metadata_sender: async_channel::Sender<Option<StreamMetadata>>,
        source: ScreencastSource,
        cursor_mode: CursorMode,
    },
    /// The [`Portal`] has requested to stop the cast with the following ID.
    StopCast { cast_id: CastId },
}

#[interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl Portal {
    #[zbus(property)]
    pub fn available_source_types(&self) -> u32 {
        (SourceType::MONITOR | SourceType::WINDOW | SourceType::VIRTUAL).bits()
    }

    #[zbus(property)]
    pub fn available_cursor_modes(&self) -> u32 {
        (CursorMode::HIDDEN | CursorMode::EMBEDDED).bits()
    }

    #[zbus(property)]
    pub fn version(&self) -> u32 {
        PORTAL_VERSION
    }

    #[tracing::instrument(parent = &self.span, skip(self))]
    async fn create_session(
        &self,
        request_handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        _app_id: String,
        _options: HashMap<&str, zvariant::Value<'_>>,
        #[zbus(object_server)] object_server: &ObjectServer,
    ) -> fdo::Result<PortalResponse<CreateSessionResult>> {
        // First insert the request interface
        object_server
            .at(&request_handle, PortalRequest::new(request_handle.clone()))
            .await?;

        let session = ScreencastSession::new(
            session_handle.clone(),
            SessionData {
                to_compositor: self.to_compositor.clone(),
                cast_id: None,
                source: None,      // lazily created when receiving metadata
                cursor_mode: None, // ^^^^
            },
            Some(SessionData::on_close),
        );
        object_server.at(&session_handle, session).await?;

        Ok(PortalResponse::Success(CreateSessionResult {
            handle_token: session_handle.to_string(),
        }))
    }

    #[tracing::instrument(parent = &self.span, skip(self))]
    async fn select_sources(
        &self,
        request_handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        _app_id: String,
        options: SelectSourcesOptions,
        #[zbus(signal_emitter)] signal_emitter: SignalEmitter<'_>,
        #[zbus(object_server)] object_server: &ObjectServer,
    ) -> fdo::Result<PortalResponse<HashMap<String, OwnedValue>>> {
        let session_ref = object_server
            .interface::<_, ScreencastSession>(&session_handle)
            .await?;
        let session = session_ref.get_mut().await;

        let cursor_mode = options
            .cursor_mode
            .and_then(CursorMode::from_bits)
            .unwrap_or(CursorMode::EMBEDDED);

        let exit_status = std::process::Command::new("fht-share-picker")
            .stdout(Stdio::piped())
            .spawn()
            .and_then(|child| child.wait_with_output());
        let source = match exit_status {
            Ok(output) if output.status.success() => {
                if output.stdout.is_empty() {
                    // The user clicked exit, and thus doesn't want to screencast anymore.
                    // No need to log anything.
                    return Ok(PortalResponse::Cancelled);
                }

                // Read the standard output, decode the JSON out of it
                serde_json::from_slice::<ScreencastSource>(&output.stdout).unwrap()
            }
            Ok(output) => {
                let code = output.status.code();
                warn!(?code, "fht-share-picker exited unsuccessfully");
                let _ = session.closed(&signal_emitter, HashMap::new()).await;
                return Ok(PortalResponse::Error);
            }
            Err(err) => {
                warn!(?err, "Failed to spawn fht-share-picker");
                let _ = session.closed(&signal_emitter, HashMap::new()).await;
                return Ok(PortalResponse::Error);
            }
        };

        session.with_data(|data| {
            data.source = Some(source);
            data.cursor_mode = Some(cursor_mode)
        });

        Ok(PortalResponse::Success(Default::default()))
    }

    #[tracing::instrument(parent = &self.span, skip(self))]
    #[allow(clippy::too_many_arguments)]
    async fn start(
        &self,
        request_handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        _app_id: String,
        _parent_window: String,
        _options: HashMap<&str, zvariant::Value<'_>>,
        #[zbus(signal_emitter)] signal_emitter: SignalEmitter<'_>,
        #[zbus(object_server)] object_server: &ObjectServer,
    ) -> fdo::Result<PortalResponse<StartResult>> {
        let session_ref = object_server
            .interface::<_, ScreencastSession>(&session_handle)
            .await?;
        let session = session_ref.get_mut().await;
        let Some((source, cursor_mode)) = session.with_data(|data| {
            data.source
                .clone()
                .and_then(|source| Some((source, data.cursor_mode?)))
        }) else {
            error!("Tried to start screencast before select_sources");
            return Ok(PortalResponse::Error);
        };

        // What we do now is ask the compositor to start the screencast.
        // In the dbus thread we block on the receiver until we receive *something*.
        //
        // Receive Some(metadata) => continue with streaming
        // receive None => something bad happened on the compositor/pipewire side, drop
        let (metadata_sender, metadata_receiver) = async_channel::unbounded();

        if let Err(err) = self.to_compositor.send(Request::StartCast {
            session_handle: session_handle.clone().into(),
            metadata_sender,
            source: source.clone(),
            cursor_mode,
        }) {
            warn!(?err, "Failed to send StartCast request to compositor");
            let _ = session.closed(&signal_emitter, HashMap::new()).await;
            return Ok(PortalResponse::Error);
        }

        let StreamMetadata {
            cast_id,
            node_id,
            size,
        } = match metadata_receiver.recv().await {
            Ok(Some(metadata)) => metadata,
            Ok(None) => {
                let _ = session.closed(&signal_emitter, HashMap::new()).await;
                return Ok(PortalResponse::Error);
            }
            Err(err) => {
                warn!(
                    ?err,
                    "Metadata receiver channel closed when it should not, weird..."
                );
                let _ = session.closed(&signal_emitter, HashMap::new()).await;
                return Ok(PortalResponse::Error);
            }
        };

        // A client should only be able to call start once per session.
        // We assert this here.
        session.with_data(|data| assert!(data.cast_id.replace(cast_id).is_none()));

        let stream_properties = StreamProperties {
            size: (size.w as i32, size.h as i32),
            source_type: (match &source {
                ScreencastSource::Output { .. } => SourceType::MONITOR,
                ScreencastSource::Window { .. } => SourceType::WINDOW,
                ScreencastSource::Workspace { .. } => SourceType::VIRTUAL,
            })
            .bits(),
        };

        // TODO: Support persist mode
        Ok(PortalResponse::Success(StartResult {
            streams: vec![Stream(node_id, stream_properties)],
            // FIXME: persistence support.
            persist_mode: 0,
            restore_token: None,
        }))
    }
}

pub struct SessionData {
    /// Channel to send [`Request`]s to the compositor.
    to_compositor: calloop::channel::Sender<Request>,
    /// The pipewire cast that is streaming for this session.
    cast_id: Option<CastId>,
    /// The source used for this session.
    source: Option<ScreencastSource>,
    /// The cursor mode used for this session.
    cursor_mode: Option<CursorMode>,
}

impl SessionData {
    fn on_close(&self) {
        if let Some(cast_id) = self.cast_id {
            if let Err(err) = self.to_compositor.send(Request::StopCast { cast_id }) {
                error!(
                    ?err,
                    ?cast_id,
                    "Failed to send StopCast request to compositor"
                );
            };
        }
    }
}

/// The metadata associated with a pipewire stream, received from the compositor.
pub struct StreamMetadata {
    pub cast_id: CastId,
    pub node_id: u32,
    pub size: Size<u32, Physical>,
}

// This enum is taken straight from fht-share-picker
// SEE: https://github.com/nferhat/fht-share-picker
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ScreencastSource {
    Window { id: usize },
    Workspace { output: String, idx: usize },
    Output { name: String },
}
