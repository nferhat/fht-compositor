//! Shared logic for XDG desktop portal.
//!
//! Using an XDG desktop portal revolves around creating a [`Session`]. The client can then either
//! call [`Session::close`] to end the portal session OR receive the `Session::closed` signal.
//!
//! When the client needs something, the application calls a portal request, receives back an
//! object path to a [`Request`] object, and when the portal backend is done, it sends out a
//! `Request::response` signal containing the data.

mod request;
use std::sync::{Arc, Mutex};

pub use request::Request as PortalRequest;
use zbus::object_server::SignalEmitter;
use zbus::ObjectServer;

/// A long-lived XDG portal session.
///
/// You can optionally associate some portal-specific data with this session. Useful to keep for
/// example communication channels to the compositor or portal state.
pub struct PortalSession<D: Session> {
    data: Arc<Mutex<D>>,
    handle: zvariant::OwnedObjectPath,
}

impl<D: Session + Send + 'static> PortalSession<D> {
    /// Create a new XDG desktop portal session.
    pub async fn init<P>(handle: P, data: D, object_server: &ObjectServer) -> zbus::Result<()>
    where
        P: Into<zvariant::OwnedObjectPath>,
    {
        let handle = handle.into();
        let iface = Self {
            data: Arc::new(Mutex::new(data)),
            handle: handle.clone(),
        };
        let new = object_server.at(handle, iface).await?;
        if !new {
            // The sessions should be unique, IE the client implementation should always generate
            // a new random handle each time it starts a new session.
            warn!("Duplicate portal session");
        }

        Ok(())
    }

    pub fn with_data<F, R>(&self, cb: F) -> R
    where
        F: FnOnce(&mut D) -> R,
    {
        let mut data = self.data.lock().unwrap();
        cb(&mut *data)
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Session")]
impl<T: Session + Send + 'static> PortalSession<T> {
    /// Closes the portal session to which this object refers and ends all related user interaction
    /// (dialogs, etc).
    pub async fn close(&mut self, #[zbus(object_server)] object_server: &zbus::ObjectServer) {
        match object_server.remove::<Self, _>(&self.handle).await {
            Ok(true) => (),
            Ok(false) => warn!(handle = ?self.handle, "Could not destroy portal session"),
            Err(err) => error!(?err, "Failed to destroy portal session"),
        }

        let mut data = self.data.lock().unwrap();
        data.on_destroy();
    }

    /// Emitted when a session is closed.
    ///
    /// The content of details is specified by the interface creating the session.
    #[zbus(signal)]
    pub async fn closed(_emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Trait for session data to implement.
pub trait Session {
    fn on_destroy(&mut self) {
        // No-op.
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
