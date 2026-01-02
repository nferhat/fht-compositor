//! A portal session request.
//!
//! This interface/object is shared by all the backend implementations. The portal backend (us)
//! will export this request object for the XDG desktop portal daemon to use. It should be kept
//! alive as long as needed.

use zbus::{Error, ObjectServer};
use zvariant::{ObjectPath, OwnedObjectPath};

/// A single portal request.
pub struct Request {
    handle: zvariant::OwnedObjectPath,
}

impl Request {
    /// Create a new XDG desktop portal request.
    pub async fn init(
        handle: impl Into<OwnedObjectPath>,
        object_server: &ObjectServer,
    ) -> zbus::Result<()> {
        let handle = handle.into();
        let iface = Self {
            handle: handle.clone(),
        };
        let new = object_server.at(handle, iface).await?;
        if !new {
            // The requests should be unique, IE the client implementation should always generate
            // a new random request handle each time it starts a new session.
            warn!("Duplicate portal request");
        }

        Ok(())
    }

    /// Remove a [`PortalRequest`] object at the given path.
    pub async fn stop<'a, P>(handle: P, object_server: &'a ObjectServer)
    where
        P: TryInto<ObjectPath<'a>>,
        P::Error: Into<Error>,
    {
        let _ = object_server.remove::<Self, _>(handle).await;
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Request")]
impl Request {
    /// Closes the portal request to which this object refers and ends all related user interaction
    /// (dialogs, etc).
    pub async fn close(&mut self, #[zbus(object_server)] object_server: &ObjectServer) {
        match object_server.remove::<Self, _>(&self.handle).await {
            Ok(true) => (),
            Ok(false) => warn!(handle = ?self.handle, "Could not destroy portal request"),
            Err(err) => error!(?err, "Failed to destroy portal request"),
        }
    }
}
