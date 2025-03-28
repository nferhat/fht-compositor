//! Inter-process communication for `fht-compositor`
//!
//! ## Interacting with the IPC
//!
//! There are three ways to interact the IPC:
//!
//! 1. Use the `fht-compositor cli` command line, which is a CLI wrapper around method number 2.
//!    Useful for writing scripts with the `-j/--json` flag or querying information and have a nice
//!    (but unstable) output.
//!
//! 2. Make programmatic use of the IPC, which gives types to use with [`serde`]. You should open up
//!    a [`UnixStream`] with [`connect`] and serialize/deserialize your requests to/from JSON.
//!
//! 3. Make use of tools like [socat](//www.dest-unreach.org/socat/) with [jq](https://jqlang.org/)
//!    for more thorough scripting purposes or just use whatever your favourite language has to
//!    offer for Unix socket communication.
//!
//! ## Using the IPC
//!
//! When it comes to **using** the IPC, you can query some information using [`Request`] and
//! get out a [`Response`].
//!
//! **TODO**: Event stream

use std::os::unix::net::UnixStream;

use anyhow::Context;

const SOCKET_DEFAULT_ENV: &'static str = "FHTC_SOCKET_PATH";

/// Connect to the `fht-compositor` IPC socket.
///
/// You will be responsible to manage this [`UnixStream`], IE. writing [`Request`]s serialized into
/// JSON using [`serde`] and reading out JSON to deserialize into [`Response`]s.
pub fn connect() -> anyhow::Result<(std::path::PathBuf, UnixStream)> {
    let socket_path = std::env::var(SOCKET_DEFAULT_ENV)
        .context("Missing FHTC_SOCKET_PATH environment variable")?;
    let socket_path = std::path::PathBuf::try_from(socket_path).context("Invalid socket path")?;
    let socket = UnixStream::connect(&socket_path).context("Missing IPC socket")?;
    Ok((socket_path, socket))
}
