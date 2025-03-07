//! Shared logic for XDG desktop portal.
//!
//! Using an XDG desktop portal revolves around creating a [`Session`]. The client can then either
//! call [`Session::close`] to end the portal session OR receive the `Session::closed` signal.
//!
//! When the client needs something, the application calls a portal request, receives back an
//! object path to a [`Request`] object, and when the portal backend is done, it sends out a
//! `Request::response` signal containing the data.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zbus::object_server::SignalEmitter;

/// A long-lived XDG portal session.
///
/// You can optionally associate some portal-specific data with this session. Useful to keep for
/// example communication channels to the compositor or portal state.
pub struct Session<T: 'static> {
    data: Arc<Mutex<T>>,
    on_destroy: Option<Box<dyn FnOnce(&T) + Send + Sync>>,
    handle: zvariant::OwnedObjectPath,
}

impl<T: 'static> Session<T> {
    /// Create a new XDG desktop portal session.
    pub fn new<P, F>(handle: P, data: T, on_destroy: Option<F>) -> Self
    where
        P: Into<zvariant::OwnedObjectPath>,
        F: FnOnce(&T) + Send + Sync + 'static,
    {
        Self {
            data: Arc::new(Mutex::new(data)),
            handle: handle.into(),
            on_destroy: on_destroy.map(|cb| Box::new(cb) as Box<_>),
        }
    }

    pub fn with_data<F, R>(&self, cb: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        let mut data = self.data.lock().unwrap();
        cb(&mut *data)
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Session")]
impl<T: Send + 'static> Session<T> {
    /// Closes the portal session to which this object refers and ends all related user interaction
    /// (dialogs, etc).
    pub async fn close(&mut self, #[zbus(object_server)] object_server: &zbus::ObjectServer) {
        match object_server.remove::<Self, _>(&self.handle).await {
            Ok(true) => (),
            Ok(false) => warn!(handle = ?self.handle, "Could not destroy portal session"),
            Err(err) => error!(?err, "Failed to destroy portal session"),
        }

        if let Some(on_destroy) = self.on_destroy.take() {
            let data = self.data.lock().unwrap();
            on_destroy(&*data);
        }
    }

    /// Emitted when a session is closed.
    ///
    /// The content of details is specified by the interface creating the session.
    #[zbus(signal)]
    pub async fn closed(
        &self,
        _emitter: &SignalEmitter<'_>,
        details: HashMap<&str, zvariant::Value<'_>>,
    ) -> zbus::Result<()>;
}

/// A single portal request.
pub struct Request {
    handle: zvariant::OwnedObjectPath,
}

impl Request {
    /// Create a new XDG desktop portal request.
    pub fn new<P>(handle: P) -> Self
    where
        P: Into<zvariant::OwnedObjectPath>,
    {
        Self {
            handle: handle.into(),
        }
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Request")]
impl Request {
    /// Closes the portal request to which this object refers and ends all related user interaction
    /// (dialogs, etc).
    pub async fn close(&mut self, #[zbus(object_server)] object_server: &zbus::ObjectServer) {
        match object_server.remove::<Self, _>(&self.handle).await {
            Ok(true) => (),
            Ok(false) => warn!(handle = ?self.handle, "Could not destroy portal request"),
            Err(err) => error!(?err, "Failed to destroy portal request"),
        }
    }
}

/// A result from a portal request.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde_repr::Deserialize_repr,
    serde_repr::Serialize_repr,
    zvariant::Type,
)]
#[repr(u32)]
pub enum PortalResponse {
    Success,
    Cancelled,
    Error,
}
