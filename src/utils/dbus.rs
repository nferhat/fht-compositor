use std::sync::LazyLock;

use zbus::blocking;

pub static DBUS_CONNECTION: LazyLock<blocking::Connection> = LazyLock::new(|| {
    let session = blocking::ConnectionBuilder::session().expect("Failed to open session bus!");
    let connection = session
        .name("fht.desktop.Compositor")
        .expect("Failed to reserve service name!");
    connection.build().unwrap()
});
