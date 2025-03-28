use std::io;
use std::os::unix::net::UnixListener;

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};

use crate::state::State;

/// The compositor IPC server.
pub struct Server {
    registration_token: RegistrationToken,
}

/// Start the [`IpcServer`] for the compositor.
pub fn start(
    loop_handle: &LoopHandle<'static, State>,
    wayland_socket_name: &str,
) -> anyhow::Result<Server> {
    let pid = std::process::id();

    // SAFETY: We place socket in XDG_RUNTIME_DIR, which should always be available to create the
    // wayland socket itself.
    let socket_dir = xdg::BaseDirectories::new()
        .unwrap()
        .get_runtime_directory()
        .cloned()
        .unwrap();
    let socket_name = format!("fhtc-{pid}-{wayland_socket_name}.socket");
    let socket_path = socket_dir.join(&socket_name);
    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;

    let generic = Generic::new(listener, Interest::READ, Mode::Level);
    let registration_token = loop_handle.insert_source(generic, |_, listener, state| {
        match listener.accept() {
            Ok((_stream, addr)) => {
                info!(?addr, "New IPC client");
                // TODO: Handle clients
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => (),
            Err(err) => return Err(err),
        }

        // TODO: Handle clients
        Ok(PostAction::Continue)
    })?;

    unsafe {
        // SAFETY: We do not have any threaded activity **yet**
        std::env::set_var("FHTC_SOCKET_PATH", &socket_path);
    }

    info!(?socket_path, "Started IPC");

    Ok(Server { registration_token })
}
