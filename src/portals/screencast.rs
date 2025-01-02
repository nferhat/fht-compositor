//! XDG screencast implementation.
//!
//! This file only handles D-Bus communication. For pipewire logic, see `src/pipewire/mod.rs`

use std::collections::HashMap;
use std::fmt::Write;
use std::io::Read;
use std::sync::atomic::AtomicUsize;

use anyhow::Context;
use smithay::reexports::calloop;
use smithay::utils::{Physical, Size};
use zbus::object_server::SignalContext;
use zbus::{interface, ObjectServer};

use crate::state::{Fht, State};
use crate::utils::pipewire::CastId;

pub const PORTAL_VERSION: u32 = 5;

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
        (SourceType::MONITOR | SourceType::WINDOW).bits()
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
    ) -> (u32, HashMap<&str, zvariant::Value<'_>>) {
        let span = make_screencast_span("create_session", &session_handle, &request_handle);
        let _span_guard = span.enter();

        let request = PortalRequest;
        if let Err(err) = object_server.at(&request_handle, request).await {
            warn!(?err, "Failed to create screencast request object");
            return (2, HashMap::new());
        };

        let id = next_session_id();
        let session = PortalSession {
            id,
            to_compositor: self.to_compositor.clone(),
            cast_id: None,
            source: None,      // lazily created when receiving metadata
            cursor_mode: None, // ^^^^
        };

        if let Err(err) = object_server.at(&session_handle, session).await {
            let _ = object_server
                .remove::<PortalRequest, _>(&request_handle)
                .await; // even we dont remove this its not really important
            warn!(?err, "Failed to create screencast session object");
            return (2, HashMap::new());
        };

        let mut session_id_string = String::new();
        write!(&mut session_id_string, "session-{}", id).unwrap();

        (
            0,
            HashMap::from_iter([("session_id", zvariant::Value::new(session_id_string))]),
        )
    }

    async fn select_sources(
        &self,
        request_handle: zvariant::ObjectPath<'_>,
        session_handle: zvariant::ObjectPath<'_>,
        _app_id: String,
        options: HashMap<&str, zvariant::Value<'_>>,
        #[zbus(signal_context)] signal_ctx: SignalContext<'_>,
        #[zbus(object_server)] object_server: &ObjectServer,
    ) -> (u32, HashMap<&str, zvariant::Value<'_>>) {
        let span = make_screencast_span("select_sources", &session_handle, &request_handle);
        let _span_guard = span.enter();

        let session_ref = object_server
            .interface::<_, PortalSession>(&session_handle)
            .await
            .expect("select_sources call should be on a valid session");
        let mut session = session_ref.get_mut().await;

        let cursor_mode = get_option_value::<u32>(&options, "cursor_mode")
            .ok()
            .and_then(CursorMode::from_bits)
            .unwrap_or_else(|| {
                warn!("Failed to get 'cursor_mode' from options, using HIDDEN");
                CursorMode::HIDDEN
            });

        let base_directories = xdg::BaseDirectories::new().unwrap();
        let output_path = base_directories
            .place_runtime_file("fht-compositor/screencast-output.json")
            .unwrap();
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&output_path)
            .unwrap();

        let exit_status = std::process::Command::new("fht-share-picker")
            .arg(&output_path)
            .spawn()
            .and_then(|mut child| child.wait());
        match exit_status {
            Ok(status) if status.success() => (),
            Ok(status) => {
                warn!(
                    code = status.code(),
                    "fht-share-picker exited unsuccessfully"
                );

                let _ = session.closed(&signal_ctx, HashMap::new()).await;
                return (2, HashMap::new());
            }
            Err(err) => {
                warn!(?err, "Failed to spawn fht-share-picker");
                let _ = session.closed(&signal_ctx, HashMap::new()).await;
                return (2, HashMap::new());
            }
        }

        let mut buf = String::new();
        match file.read_to_string(&mut buf) {
            Ok(_) => (),
            Err(err) => {
                warn!(?err, "Failed to read fht-share-picker results");
                let _ = session.closed(&signal_ctx, HashMap::new()).await;
                return (2, HashMap::new());
            }
        }

        let source =
            serde_json::de::from_str(&buf).expect("fht-share-picker should give valid JSON!");
        session.source = Some(source);
        session.cursor_mode = Some(cursor_mode);

        (0, HashMap::new())
    }

    async fn start(
        &self,
        request_handle: zvariant::ObjectPath<'_>,
        session_handle: zvariant::ObjectPath<'_>,
        _app_id: String,
        _parent_window: String,
        _options: HashMap<&str, zvariant::Value<'_>>,
        #[zbus(signal_context)] signal_ctx: SignalContext<'_>,
        #[zbus(object_server)] object_server: &ObjectServer,
    ) -> (u32, HashMap<&str, zvariant::Value<'_>>) {
        let span = make_screencast_span("start", &session_handle, &request_handle);
        let _span_guard = span.enter();

        let session_ref = object_server
            .interface::<_, PortalSession>(&session_handle)
            .await
            .unwrap();
        let mut session = session_ref.get_mut().await;

        let source = session
            .source
            .clone()
            .expect("a session can only start after select_sources");
        let cursor_mode = session
            .cursor_mode
            .clone()
            .expect("a session can only start after select_sources");

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
            let _ = session.closed(&signal_ctx, HashMap::new()).await;
            return (2, HashMap::new());
        }

        let StreamMetadata {
            cast_id,
            node_id,
            size,
        } = match metadata_receiver.recv().await {
            Ok(Some(metadata)) => metadata,
            Ok(None) => {
                let _ = session.closed(&signal_ctx, HashMap::new()).await;
                return (1, HashMap::new());
            }
            Err(err) => {
                warn!(
                    ?err,
                    "Metadata receiver channel closed when it should not, weird..."
                );
                let _ = session.closed(&signal_ctx, HashMap::new()).await;
                return (2, HashMap::new());
            }
        };

        assert!(session.cast_id.replace(cast_id).is_none());
        let mut results = HashMap::new();

        results.insert(
            "streams",
            zvariant::Value::new(vec![(node_id, {
                let mut response = HashMap::new();
                // NOTE: Even we don't include position, it doesn't seem to be used at all
                response.insert("size", zvariant::Value::new((size.w, size.h)));
                response.insert(
                    "source_type",
                    zvariant::Value::new(match &source {
                        ScreencastSource::Output { .. } => SourceType::MONITOR.bits(),
                        ScreencastSource::Window { .. } => SourceType::WINDOW.bits(),
                        ScreencastSource::Workspace { .. } => SourceType::VIRTUAL.bits(),
                    }),
                );

                response
            })]),
        );
        // TODO: Support persist mode
        results.insert("persist_mode", zvariant::Value::U32(0));

        // as per the portal API documentation,
        // return 0 as the status code, and results contain our node metadata
        (0, results)
    }
}

static SESSION_IDS: AtomicUsize = AtomicUsize::new(0);
fn next_session_id() -> usize {
    SESSION_IDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

/// A single screencast session.
///
/// Read more <https://flatpak.github.io/xdg-desktop-portal/docs/sessions.html>
pub struct PortalSession {
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

#[interface(name = "org.freedesktop.impl.portal.Session")]
impl PortalSession {
    pub fn close(&self) {
        if let Some(cast_id) = self.cast_id {
            if let Err(err) = self.to_compositor.send(Request::StopCast { cast_id }) {
                warn!(?err, "Failed to send StopCast request to compositor");
            }
        }
    }

    #[zbus(signal)]
    pub async fn closed(
        &self,
        signal_ctx: &SignalContext<'_>,
        details: HashMap<&str, zvariant::Value<'_>>,
    ) -> zbus::Result<()>;
}

/// A singla screencast request.
///
/// NOTE: This does nothing much, expect being compliant with the portal interface? Even what
/// xdg-desktop-portal-wlr does is just create it and forget about it.
///
/// Read more <https://flatpak.github.io/xdg-desktop-portal/docs/requests.html>
pub struct PortalRequest;

#[interface(name = "org.freedesktop.portal.Request")]
impl PortalRequest {
    /// Closes the portal request to which this object refers and ends all related user interaction
    /// (dialogs, etc).
    ///
    /// A [`Self::response`] signal will not be emitted in this case.
    fn close(&self) {}

    /// Emitted when the user interaction for a portal request is over.
    #[zbus(signal)]
    async fn response(
        &self,
        signal_ctx: &SignalContext<'_>,
        reason: u32,
        results: HashMap<&str, zvariant::Value<'_>>,
    ) -> zbus::Result<()>;
}

// This enum is taken straight from fht-share-picker
// SEE: https://github.com/nferhat/fht-share-picker
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ScreencastSource {
    Window { foreign_toplevel_handle: String },
    Workspace { output: String, idx: usize },
    Output { name: String },
}

impl State {}

impl Fht {}

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
    let session_handle = session_handle.to_string();
    let session_handle = session_handle
        .strip_prefix("/org/freedesktop/portal/desktop/session/")
        .expect("session handle should always contain prefix");
    let request_handle = request_handle.to_string();
    let request_handle = request_handle
        .strip_prefix("/org/freedesktop/portal/desktop/request/")
        .expect("request handle should always contain prefix");

    let span = debug_span!(
        "screencast",
        event = tracing::field::Empty,
        session = tracing::field::Empty,
        request = tracing::field::Empty,
    );
    span.record("event", event);
    span.record("session", session_handle);
    span.record("request", request_handle);

    span
}
