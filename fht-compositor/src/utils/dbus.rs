use std::sync::LazyLock;

use zbus::blocking;

/// The session connection of the compositor to ensure the `fht.desktop.Compositor` service name on
/// the session connection.
///
/// This gets initialized whenever we use the CONNECTION (so everywhere), the thing is it WILL
/// crash if the compositor isn't ran under a dbus session.
pub static DBUS_CONNECTION: LazyLock<blocking::Connection> = LazyLock::new(|| {
    let session = blocking::ConnectionBuilder::session().expect("Failed to open session bus!");
    let connection = session
        .name("fht.desktop.Compositor")
        .expect("Failed to reserve service name!");
    connection.build().unwrap()
});
