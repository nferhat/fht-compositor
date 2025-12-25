//! XDG screencast implementation.
//!
//! This file only handles D-Bus communication. For pipewire logic, see `src/pipewire/mod.rs`

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use anyhow::Context;
use smithay::reexports::calloop;
use smithay::utils::{Physical, Size};
use zbus::object_server::SignalEmitter;
use zbus::{interface, ObjectServer};

use crate::utils::get_monotonic_time;
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
    /// Check/make sure that a [`ScreencastSource`] is valid.
    CheckSource {
        source: ScreencastSource,
        results_render: async_channel::Sender<bool>,
    },
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
            persist: 0,
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
        let persist = get_option_value::<u32>(&options, "persist_mode").unwrap_or(0);

        #[allow(unused_assignments)]
        let mut source = None;

        // Before running fht-share-picker, we try to restore the previously selected source. When
        // encoding it the restore_data, we make sure that its
        // 1. Not too old
        // 2. Still valid (requires checking with the compositor), since for example, the client
        //    might be asking to restore a source of a dead window ID.
        if let Some(restore_data) =
            get_option_value::<zvariant::Structure>(&options, "restore_data").ok()
        {
            trace!(?restore_data, "Got screencast session restore data");
            // The restore data takes the form of (VENDOR_ID, FORMAT_VERSION, DATA)
            // - VENDOR_ID is always harcoded to the string "fht-compositor"
            // - FORMAT_VERSION is used for backwards compatibility if we make changes to DATA.
            // - DATA is whatever's needed to restore the session.
            let fields = restore_data.into_fields();

            if fields.len() == 3
                && fields[0].to_string() == "fht-compositor"
                && fields[1].downcast_ref::<u32>().unwrap() == 1
            {
                // SAFETY: We are assured the RestoreData passed in from the client is the same as
                // the restore data we are expecting
                let dict = fields[2].try_to_owned().unwrap();
                let RestoreData {
                    window_id,
                    output_name,
                    workspace_idx,
                    created_at,
                } = RestoreData::try_from(dict).unwrap();

                // Thank you zbus for now being able to store time
                let created_at = Duration::from_secs(created_at);
                let now = get_monotonic_time();

                if now.saturating_sub(created_at) < RESTORE_DATA_LIFETIME {
                    if let Some(window_id) = window_id {
                        source = Some(ScreencastSource::Window {
                            id: window_id as usize,
                        });
                    } else if let Some(output_name) = output_name {
                        source = Some(match workspace_idx {
                            Some(idx) => ScreencastSource::Workspace {
                                output: output_name,
                                idx: idx as _,
                            },
                            None => ScreencastSource::Output { name: output_name },
                        });
                    }

                    trace!(?source, "Screencast source from restore_data");
                } else {
                    trace!(?source, "Ignoring restore_data since its too old");
                }
            }
        }

        // Now make sure the screencast source is actually valid.
        source.take_if(|source| {
            let (tx, rx) = async_channel::bounded(1);
            if let Err(err) = self.to_compositor.send(Request::CheckSource {
                source: source.clone(),
                results_render: tx,
            }) {
                warn!(?err, "Failed to send CheckSource request to compositor");
                return true; // can't check the source, just pick a new one
            }

            match rx.recv_blocking() {
                Ok(res) => !res,
                Err(err) => {
                    warn!(?err, "CheckSource channel closed, weird...");
                    true // can't check the source, just pick a new one
                }
            }
        });

        // If we didn't get a source yet (IE no restore_data, or invalid/old one), proceed to open
        // fht-share-picker to prompt the user for a screencast source
        if source.is_none() {
            let exit_status = std::process::Command::new("fht-share-picker")
                .stdout(Stdio::piped())
                .spawn()
                .and_then(|child| child.wait_with_output());
            source = Some(match exit_status {
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
            });

            trace!(?source, "Got source from fht-share-picker");
        }

        let Some(source) = source else {
            warn!("Failed to get screencast source");
            let _ = session.closed(&signal_emitter, HashMap::new()).await;
            return (PortalResponse::Error, HashMap::new());
        };

        session.with_data(|data| {
            data.source = Some(source);
            data.cursor_mode = Some(cursor_mode);
            data.persist = persist; // 1 = until app closes, 2 = until we revoke it.
                                    // FIXME: Handle until app closes better?
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

        let mut results = HashMap::from_iter([("streams", zvariant::Value::new(vec![stream]))]);
        let persist_mode = session.with_data(|data| data.persist);
        if persist_mode > 0 {
            // Create persistence data.
            let restore_data = match source.clone() {
                ScreencastSource::Window { id } => RestoreData::window(id),
                ScreencastSource::Workspace { output, idx } => RestoreData::workspace(output, idx),
                ScreencastSource::Output { name } => RestoreData::output(name),
            };
            let restore_data = zvariant::StructureBuilder::new()
                .add_field("fht-compositor")
                .add_field(1u32)
                // This weird conversion transform restore_data from a{sv} -> v
                // IE turn it into a plain variant instead of a dict
                .append_field(zvariant::Value::Value(Box::new(restore_data.into())))
                .build()
                .unwrap();
            results.insert("persist_mode", zvariant::Value::U32(persist_mode));
            results.insert("restore_data", zvariant::Value::from(restore_data));
        }

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
    /// Whether we should persist this session.
    persist: u32,
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

// Give restore data tokens a quite generous 5 minute timeout.
// FIXME: Maybe add an option in debug config?
const RESTORE_DATA_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// Restore data send/received by clients to keep sessions similar.
#[derive(Debug, Clone, zvariant::Value, zvariant::OwnedValue, zvariant::Type)]
#[zvariant(signature = "dict", rename_all = "snake-case")]
struct RestoreData {
    // Since we can't just put a [`ScreencastSource`], we do the following
    // 1. If window_id exists, assume it was a window source
    // 2. If output_name exists AND workspace_idx exists, assume it was a workspace source
    // 3. If only output_name exists, assume it was a output source.
    window_id: Option<u32>,
    output_name: Option<String>,
    workspace_idx: Option<u32>,
    /// Give a timeout for restore data. Anything that's lived for longer than
    /// [`RESTORE_DATA_LIFETIME`] gets invalidated. And since zbus can't serialize instants, we
    /// store the UNIX_EPOCH time.
    created_at: u64,
}

impl RestoreData {
    fn window(id: usize) -> Self {
        Self {
            window_id: Some(id as u32),
            output_name: None,
            workspace_idx: None,
            created_at: get_monotonic_time().as_secs(),
        }
    }

    fn workspace(output: String, idx: usize) -> Self {
        Self {
            window_id: None,
            output_name: Some(output),
            workspace_idx: Some(idx as u32),
            created_at: get_monotonic_time().as_secs(),
        }
    }

    fn output(output: String) -> Self {
        Self {
            window_id: None,
            output_name: Some(output),
            workspace_idx: None,
            created_at: get_monotonic_time().as_secs(),
        }
    }
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
