//! XDG screencast implementation.
//!
//! This file only handles D-Bus communication. For pipewire logic, see `src/pipewire/mod.rs`

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::AtomicUsize;

use anyhow::Context;
use smithay::reexports::calloop;
use smithay::utils::{Physical, Size};
use zbus::object_server::SignalEmitter;
use zbus::{interface, ObjectServer};

use crate::utils::pipewire::CastId;

pub const PORTAL_VERSION: u32 = 5;

pub type ScreencastSession = super::shared::Session<SessionData>;
use super::shared::{PortalResponse, Request as ScreencastRequest};

bitflags::bitflags! {
    #[derive(Clone, Copy, PartialEq)]
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
    pub(super) to_compositor: calloop::channel::Sender<Request>,
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

    async fn create_session(
        &self,
        request_handle: zvariant::ObjectPath<'_>,
        session_handle: zvariant::ObjectPath<'_>,
        _app_id: String,
        _options: HashMap<&str, zvariant::Value<'_>>,
        #[zbus(object_server)] object_server: &ObjectServer,
    ) -> (PortalResponse, HashMap<&str, zvariant::Value<'_>>) {
        let span = make_screencast_span("create_session", &session_handle, &request_handle);
        let _span_guard = span.enter();

        let request = ScreencastRequest::new(request_handle.clone());
        if let Err(err) = object_server.at(&request_handle, request).await {
            warn!(?err, "Failed to create screencast request object");
            return (PortalResponse::Error, HashMap::new());
        };

        let session_data = SessionData {
            id: next_session_id(),
            to_compositor: self.to_compositor.clone(),
            cast_id: None,
            source: None,      // lazily created when receiving metadata
            cursor_mode: None, // ^^^^
        };
        let session_id = format!("fht-compositor-screencast-{}", session_data.id);

        let session = ScreencastSession::new(
            session_handle.clone(),
            session_data,
            Some(|data: &SessionData| {
                if let Some(cast_id) = data.cast_id {
                    if let Err(err) = data.to_compositor.send(Request::StopCast { cast_id }) {
                        error!(
                            ?err,
                            ?cast_id,
                            "Failed to send StopCast request to compositor"
                        );
                    };
                }
            }),
        );

        if let Err(err) = object_server.at(&session_handle, session).await {
            let _ = object_server
                .remove::<ScreencastRequest, _>(&request_handle)
                .await; // even we dont remove this its not really important
            warn!(?err, "Failed to create screencast session object");
            return (PortalResponse::Error, HashMap::new());
        };

        let results = HashMap::from_iter([("session_id", session_id.into())]);
        (PortalResponse::Success, results)
    }

    async fn select_sources(
        &self,
        request_handle: zvariant::ObjectPath<'_>,
        session_handle: zvariant::ObjectPath<'_>,
        _app_id: String,
        options: HashMap<&str, zvariant::Value<'_>>,
        #[zbus(signal_emitter)] signal_emitter: SignalEmitter<'_>,
        #[zbus(object_server)] object_server: &ObjectServer,
    ) -> (PortalResponse, HashMap<&str, zvariant::Value<'_>>) {
        let span = make_screencast_span("select_sources", &session_handle, &request_handle);
        let _span_guard = span.enter();

        let session_ref = object_server
            .interface::<_, ScreencastSession>(&session_handle)
            .await
            .expect("select_sources call should be on a valid session");
        let session = session_ref.get_mut().await;

        let cursor_mode = get_option_value::<u32>(&options, "cursor_mode")
            .ok()
            .and_then(CursorMode::from_bits)
            .unwrap_or_else(|| {
                warn!("Failed to get 'cursor_mode' from options, using HIDDEN");
                CursorMode::HIDDEN
            });

        let exit_status = std::process::Command::new("fht-share-picker")
            .stdout(Stdio::piped())
            .spawn()
            .and_then(|child| child.wait_with_output());
        let source = match exit_status {
            Ok(output) if output.status.success() => {
                if output.stdout.is_empty() {
                    // The user clicked exit, and thus doesn't want to screencast anymore.
                    // No need to log anything.
                    return (PortalResponse::Cancelled, HashMap::new());
                }

                // Read the standard output, decode the JSON out of it
                serde_json::from_slice::<ScreencastSource>(&output.stdout).unwrap()
            }
            Ok(output) => {
                let code = output.status.code();
                warn!(?code, "fht-share-picker exited unsuccessfully");
                let _ = session.closed(&signal_emitter, HashMap::new()).await;
                return (PortalResponse::Error, HashMap::new());
            }
            Err(err) => {
                warn!(?err, "Failed to spawn fht-share-picker");
                let _ = session.closed(&signal_emitter, HashMap::new()).await;
                return (PortalResponse::Error, HashMap::new());
            }
        };

        session.with_data(|data| {
            data.source = Some(source);
            data.cursor_mode = Some(cursor_mode)
        });

        (PortalResponse::Success, HashMap::new())
    }

    #[allow(clippy::too_many_arguments)]
    async fn start(
        &self,
        request_handle: zvariant::ObjectPath<'_>,
        session_handle: zvariant::ObjectPath<'_>,
        _app_id: String,
        _parent_window: String,
        _options: HashMap<&str, zvariant::Value<'_>>,
        #[zbus(signal_emitter)] signal_emitter: SignalEmitter<'_>,
        #[zbus(object_server)] object_server: &ObjectServer,
    ) -> (PortalResponse, HashMap<&str, zvariant::Value<'_>>) {
        let span = make_screencast_span("start", &session_handle, &request_handle);
        let _span_guard = span.enter();

        let session_ref = object_server
            .interface::<_, ScreencastSession>(&session_handle)
            .await
            .unwrap();
        let session = session_ref.get_mut().await;
        let Some((source, cursor_mode)) = session.with_data(|data| {
            data.source
                .clone()
                .and_then(|source| Some((source, data.cursor_mode?)))
        }) else {
            error!("Tried to start screencast before select_sources");
            return (PortalResponse::Error, HashMap::new());
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
            return (PortalResponse::Error, HashMap::new());
        }

        let StreamMetadata {
            cast_id,
            node_id,
            size,
        } = match metadata_receiver.recv().await {
            Ok(Some(metadata)) => metadata,
            Ok(None) => {
                let _ = session.closed(&signal_emitter, HashMap::new()).await;
                return (PortalResponse::Error, HashMap::new());
            }
            Err(err) => {
                warn!(
                    ?err,
                    "Metadata receiver channel closed when it should not, weird..."
                );
                let _ = session.closed(&signal_emitter, HashMap::new()).await;
                return (PortalResponse::Error, HashMap::new());
            }
        };

        // A client should only be able to call start once per session.
        // We assert this here.
        session.with_data(|data| assert!(data.cast_id.replace(cast_id).is_none()));

        let size = zvariant::Value::new((size.w, size.h));
        let source_type = zvariant::Value::new(match &source {
            ScreencastSource::Output { .. } => SourceType::MONITOR.bits(),
            ScreencastSource::Window { .. } => SourceType::WINDOW.bits(),
            ScreencastSource::Workspace { .. } => SourceType::VIRTUAL.bits(),
        });
        let stream_info: HashMap<_, _, std::hash::BuildHasherDefault<std::hash::DefaultHasher>> =
            HashMap::from_iter([("size", size), ("source_type", source_type)]);
        let stream = (node_id, stream_info);

        // TODO: Support persist mode
        let results = HashMap::from_iter([
            ("streams", zvariant::Value::new(vec![stream])),
            ("persist_mode", zvariant::Value::new("")),
        ]);

        // as per the portal API documentation,
        // return 0 as the status code, and results contain our node metadata
        (PortalResponse::Success, results)
    }
}

static SESSION_IDS: AtomicUsize = AtomicUsize::new(0);
fn next_session_id() -> usize {
    SESSION_IDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

pub struct SessionData {
    /// The unique ID of this [`ScreenCastSession`].
    #[allow(dead_code)]
    id: usize,
    /// Channel to send [`Request`]s to the compositor.
    to_compositor: calloop::channel::Sender<Request>,
    /// The pipewire cast that is streaming for this session.
    cast_id: Option<CastId>,
    /// The source used for this session.
    source: Option<ScreencastSource>,
    /// The cursor mode used for this session.
    cursor_mode: Option<CursorMode>,
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

fn get_option_value<'value, T: TryFrom<&'value zvariant::Value<'value>>>(
    options: &'value HashMap<&str, zvariant::Value<'value>>,
    name: &str,
) -> anyhow::Result<T> {
    T::try_from(options.get(name).context("Failed to get value!")?)
        .map_err(|_| anyhow::anyhow!("Failed to convert value!"))
}

// A simpler way to record spans.
fn make_screencast_span<'a>(
    event: &'static str,
    session_handle: &zvariant::ObjectPath<'a>,
    request_handle: &zvariant::ObjectPath<'a>,
) -> tracing::Span {
    let session_handle = session_handle
        .as_str()
        .strip_prefix("/org/freedesktop/portal/desktop/session/")
        .expect("session handle should always contain prefix");
    let request_handle = request_handle
        .as_str()
        .strip_prefix("/org/freedesktop/portal/desktop/request/")
        .expect("request handle should always contain prefix");
    let span = debug_span!("screencast", ?event, ?session_handle, ?request_handle);

    span
}
