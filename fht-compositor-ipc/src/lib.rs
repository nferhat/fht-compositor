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

use std::collections::HashMap;
use std::os::unix::net::UnixStream;

use anyhow::Context;
use serde::{Deserialize, Serialize};

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

/// A request you send to the compositor.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Request {
    /// Request the version information of the running `fht-compositor` instance.
    Version,
    /// Request information about the connected outputs.
    Outputs,
}

/// A respose from the compositor.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Response {
    /// Version information about the running `fht-compositor` instance.
    Version(String),
    /// Output information.
    Outputs(HashMap<String, Output>),
}

/// A single output.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Output {
    /// Name of the output.
    pub name: String,
    /// The output manufacturer.
    pub make: String,
    /// The output model.
    pub model: String,
    /// Serial of the output, if known.
    pub serial: Option<String>,
    /// Physical width and height of the output in mm.
    pub physical_size: Option<(u32, u32)>,
    /// Available modes for the output.
    pub modes: Vec<OutputMode>,
    /// Active mode index. If `None` there's no modes for this output.
    pub active_mode_idx: Option<usize>,
    /// Logical position.
    pub position: (i32, i32),
    /// The dimensions in logical pixels
    pub size: (u32, u32),
    /// Scale factor.
    pub scale: i32,
    /// Transform.
    pub transform: OutputTransform,
}

/// Output mode.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct OutputMode {
    /// The dimensions in physical pixels.
    pub dimensions: (u32, u32),
    /// The refresh rate in hertz.
    pub refresh: f64,
    /// Whether this mode is the preferred mode.
    pub preferred: bool,
}

#[derive(Default, Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum OutputTransform {
    #[default]
    #[serde(rename = "normal")]
    Normal,
    #[serde(rename = "90")]
    _90,
    #[serde(rename = "180")]
    _180,
    #[serde(rename = "270")]
    _270,
    #[serde(rename = "flipped")]
    Flipped,
    #[serde(rename = "flipped-90")]
    Flipped90,
    #[serde(rename = "flipped-180")]
    Flipped180,
    #[serde(rename = "flipped-270")]
    Flipped270,
}

impl From<smithay::utils::Transform> for OutputTransform {
    fn from(value: smithay::utils::Transform) -> Self {
        match value {
            smithay::utils::Transform::Normal => OutputTransform::Normal,
            smithay::utils::Transform::_90 => OutputTransform::_90,
            smithay::utils::Transform::_180 => OutputTransform::_180,
            smithay::utils::Transform::_270 => OutputTransform::_270,
            smithay::utils::Transform::Flipped => OutputTransform::Flipped,
            smithay::utils::Transform::Flipped90 => OutputTransform::Flipped90,
            smithay::utils::Transform::Flipped180 => OutputTransform::Flipped180,
            smithay::utils::Transform::Flipped270 => OutputTransform::Flipped270,
        }
    }
}
