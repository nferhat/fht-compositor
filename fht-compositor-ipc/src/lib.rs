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
    /// Request information about all mapped windows.
    Windows,
    /// Request information about the workspace system.
    Space,
}

/// A respose from the compositor.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Response {
    /// Version information about the running `fht-compositor` instance.
    Version(String),
    /// Output information.
    Outputs(HashMap<String, Output>),
    /// All windows information.
    Windows(Vec<Window>),
    /// Space information.
    Space(Space),
    /// Noop, for requests that do not need a result/output.
    Noop,
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

/// A single window.
///
/// A window is a mapped onto the screen inside a [`Workspace`]. A [`Workspace`] is managed inside a
/// [`Monitor`]. A window can't exist on two workspaces/monitors at the same time.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Window {
    /// The unique ID of this window. It is used to make requests regarding this specific window.
    pub id: usize,
    /// The title of this window. The title is an optional short string of text describing the
    /// window contents, or the window application title.
    pub title: Option<String>,
    /// The application ID of this window. An app-id lets you know what application is running. For
    /// example:
    /// - Alacritty windows will always have the app-id: `Alacritty`
    /// - GNOME apps will take the pattern: `org.gnome.*`
    ///
    /// It is **not** unique! And you should make no assumptions about its contents.
    pub app_id: Option<String>,
    /// The output this window is on.
    pub output: String,
    /// The workspace index this window is on.
    pub workspace_idx: usize,
    /// The workspace ID this window is on.
    pub workspace_id: usize,
    /// The size of this window.
    ///
    /// This is the effective size of the window, not the animated one.
    pub size: (u32, u32),
    /// The position of this window relative to the [`Monitor`]/[`Workspace`] its mapped onto. It
    /// does not represent the window position when actually being rendered!
    ///
    /// This is the effective location of the window, not the animated one.
    pub location: (i32, i32),
    /// Whether this window is fullscreened.
    pub fullscreened: bool,
    /// Whether this window is maximized.
    pub maximized: bool,
    /// Whether this window is tiled.
    pub tiled: bool,
    /// Whether this window is activated. This does not mean that this window is the *focused* one.
    /// There can be multiple activated windows on different workspaces, but there's can be at
    /// most only one focused window.
    pub activated: bool,
    /// Whether this window is focused. This means that the window is currently receiving keyboard
    /// input. Make sure to read [`Window::activated`].
    pub focused: bool,
}

/// A single workspace.
///
/// A workspace is a container of windows. It manages them and organizes them.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Workspace {
    /// The unique ID of this workspace. It is used to make requests regarding this specific
    /// workspace.
    pub id: usize,
    /// The [`Output`] this workspace belongs to.
    pub output: String,
    /// The [`Window`] list of this workspace.
    pub windows: Vec<Window>,
    /// The active window index.
    ///
    /// If there's a fullscreened window,
    /// [`Workspace::active_window_index`] will be equal to
    /// [`Workspace::fullscreen_window_idx`]
    ///
    /// If this is `None`, there are no [`Window`]s in this workspace.
    pub active_window_idx: Option<usize>,
    /// The fullscreened window index
    ///
    /// If this is `None`, there is no fullscreened [`Window`]s in this workspace.
    pub fullscreen_window_idx: Option<usize>,
    /// The master width factor.
    ///
    /// It is used in order to determine how much screen real estate should the master take up,
    /// relative to the slave stack.
    pub mwfact: f64,
    /// The number of clients in the master stack.
    ///
    /// This must NEVER be 0.
    pub nmaster: usize,
}

const WORKSPACE_COUNT: usize = 9;

/// A single monitor.
///
/// A monitor is a representation of an [`Output`] in the [`Workspace`] system. Each monitor is a
/// view onto a single workspace at a time.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Monitor {
    /// The output associated with this monitor.
    pub output: String,
    /// The workspaces associated with this monitor.
    pub workspaces: [Workspace; WORKSPACE_COUNT],
    /// The active workspace index.
    pub active_workspace_idx: usize,
    /// Whether this monitor is the active/focused monitor.
    pub active: bool,
}

/// Space information.
///
/// The space is the area containing all [`Monitor`] that organizes and orders them. When something
/// mentions "global coordinates", it means the *logical* coordinate space inside this space.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Space {
    /// The [`Monitor`]s tracked by the [`Space`]
    pub monitors: Vec<Monitor>,
    /// The index of the primary [`Monitor`].
    ///
    /// Usually this is the first added [`Monitor`]. In case the primary [`Monitor`] gets removed,
    /// this index is incremented by one.
    pub primary_idx: usize,
    /// The index of the active [`Monitor`].
    ///
    /// This should be the monitor that has the pointer cursor in its bounds.
    pub active_idx: usize,
}
