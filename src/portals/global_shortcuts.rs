use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use zbus::object_server::SignalEmitter;
use zbus::{interface, ObjectServer};
use zvariant::Str;

use super::shared::{PortalRequest, PortalResponse, PortalSession, Session};
type GsSession = PortalSession<SessionData>;

pub const PORTAL_VERSION: u32 = 2;

/// A [XDG GlobalShortcuts portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.impl.portal.GlobalShortcuts.html) instance
///
/// This structure can be added inside a zbus [`Connection`] to register the
/// `org.freedesktop.impl.portal.GlobalShortcuts` interface
pub struct Portal {
    #[allow(unused)]
    to_compositor: calloop::channel::Sender<Request>,
    span: tracing::Span,
}

impl Portal {
    pub fn new(to_compositor: calloop::channel::Sender<Request>) -> Self {
        Self {
            to_compositor,
            span: debug_span!("global-shortcuts"),
        }
    }
}

/// A [`Request`] that the [`Portal`] or a [`Session`] can send to the compositor.
pub enum Request {}

#[interface(name = "org.freedesktop.impl.portal.GlobalShortcuts")]
impl Portal {
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
        let _span = self.span.enter();

        if let Err(err) = PortalRequest::init(request_handle.clone(), object_server).await {
            error!(?err, "Failed to create portal request object");
            return (PortalResponse::Error, HashMap::new());
        };

        let session_data = SessionData {
            shortcuts: HashMap::new(),
        };
        if let Err(err) =
            PortalSession::init(session_handle.clone(), session_data, object_server).await
        {
            _ = PortalRequest::stop(request_handle, object_server);
            warn!(?err, "Failed to create screencast session object");
            return (PortalResponse::Error, HashMap::new());
        }

        (PortalResponse::Success, HashMap::new())
    }

    async fn bind_shortcuts(
        &self,
        _request_handle: zvariant::ObjectPath<'_>,
        session_handle: zvariant::ObjectPath<'_>,
        shortcuts: HashMap<&str, HashMap<&str, zvariant::Value<'_>>>,
        _parent_window: String,
        _options: HashMap<&str, zvariant::Value<'_>>,
        #[zbus(object_server)] object_server: &zbus::ObjectServer,
    ) -> (PortalResponse, HashMap<&str, zvariant::Value<'_>>) {
        let _span = self.span.enter();

        let session_ref = object_server
            .interface::<_, GsSession>(&session_handle)
            .await
            .expect("select_sources call should be on a valid session");
        let session = session_ref.get_mut().await;

        let shortcuts = dbg!(parse_shortcuts(shortcuts));
        session.with_data(|data| {
            // NOTE: Applications should always try to provide unique ID, but we still check for
            // that anyway (for debugging faulty clients)
            for (new_id, new_shortcut) in shortcuts {
                match data.shortcuts.entry(new_id.clone()) {
                    Entry::Occupied(mut occupied_entry) => {
                        warn!(id = ?new_id, "Duplicate global-shortcut ID");
                        occupied_entry.insert(new_shortcut);
                    }
                    Entry::Vacant(vacant_entry) => {
                        debug!(?new_id, "Registered new global-shortcut");
                        vacant_entry.insert(new_shortcut);
                    }
                }
            }
        });

        todo!()
    }

    async fn list_shortcuts(
        &self,
        _request_handle: zvariant::ObjectPath<'_>,
        session_handle: zvariant::ObjectPath<'_>,
        #[zbus(object_server)] object_server: &zbus::ObjectServer,
    ) -> (PortalResponse, HashMap<&str, zvariant::Value<'_>>) {
        let session_ref = object_server
            .interface::<_, GsSession>(&session_handle)
            .await
            .expect("select_sources call should be on a valid session");
        let session = session_ref.get_mut().await;

        let mut ret = HashMap::new();
        session.with_data(|data| {
            for (id, shortcut) in &data.shortcuts {
                let mut shortcut_map = HashMap::new();
                if let Some(description) = &shortcut.description {
                    shortcut_map.insert(
                        "description",
                        zvariant::Value::Str(Str::from(description.to_string())),
                    );
                }

                if let Some(preferred_trigger) = &shortcut.preferred_trigger {
                    shortcut_map.insert(
                        "preferred_trigger",
                        zvariant::Value::Str(Str::from(preferred_trigger.to_string())),
                    );
                }

                ret.insert(id.to_string(), shortcut_map);
            }
        });

        (PortalResponse::Success, HashMap::new())
    }

    async fn configure_shortcuts(
        &self,
        session_handle: zvariant::ObjectPath<'_>,
        _options: HashMap<&str, zvariant::Value<'_>>,
    ) {
        let _span = self.span.enter();
        debug!(?session_handle, "ConfigureShortcuts")
    }

    #[zbus(signal)]
    pub async fn activated(
        _emitter: &SignalEmitter<'_>,
        session_handle: &zvariant::ObjectPath<'_>,
        shortcut_id: String,
        timestamp: u64,
        options: HashMap<&str, zvariant::Value<'_>>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn deactivated(
        _emitter: &SignalEmitter<'_>,
        session_handle: &zvariant::ObjectPath<'_>,
        shortcut_id: String,
        timestamp: u64,
        options: HashMap<&str, zvariant::Value<'_>>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn shortcuts_changed(
        _emitter: &SignalEmitter<'_>,
        _session_handle: &zvariant::ObjectPath<'_>,
        shortcuts: HashMap<&str, HashMap<&str, zvariant::Value<'_>>>,
    ) -> zbus::Result<()>;
}

struct SessionData {
    shortcuts: HashMap<Arc<str>, Shortcut>,
}
impl Session for SessionData {}

/// A single shortcut.
#[derive(Debug)]
struct Shortcut {
    description: Option<Arc<str>>,
    preferred_trigger: Option<Arc<str>>,
}

fn parse_shortcuts(
    raw: HashMap<&str, HashMap<&str, zvariant::Value<'_>>>,
) -> HashMap<Arc<str>, Shortcut> {
    let mut shortcuts = HashMap::new();

    for (id, values) in raw {
        let id = id.into();
        let shortcut = Shortcut {
            description: get_option_value::<String>(&values, "description")
                .ok()
                .map(Into::into),
            preferred_trigger: get_option_value::<String>(&values, "preferred_trigger")
                .ok()
                .map(Into::into),
        };
        shortcuts.insert(id, shortcut);
    }

    shortcuts
}

fn get_option_value<'value, T: TryFrom<&'value zvariant::Value<'value>>>(
    options: &'value HashMap<&str, zvariant::Value<'value>>,
    name: &str,
) -> anyhow::Result<T> {
    T::try_from(options.get(name).context("Failed to get value!")?)
        .map_err(|_| anyhow::anyhow!("Failed to convert value!"))
}
