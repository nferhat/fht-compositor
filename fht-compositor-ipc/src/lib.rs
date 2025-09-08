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
//! The IPC also supports Event Streaming.
//!
//! To use it, `--subscribe` command is used.
//!
//! **Example:**
//!
//! ```sh
//! # Supported requests: workspace, windows, window, space, layer-shells
//! $ fht-compositor ipc --subscribe workspace
//! # ... requests will stream when internal data changes
//! ```

use std::collections::HashMap;
use std::os::unix::net::UnixStream;

use anyhow::Context;
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

const SOCKET_DEFAULT_ENV: &str = "FHTC_SOCKET_PATH";

/// Connect to the `fht-compositor` IPC socket.
///
/// You will be responsible to manage this [`UnixStream`], IE. writing [`Request`]s serialized into
/// JSON using [`serde`] and reading out JSON to deserialize into [`Response`]s.
pub fn connect() -> anyhow::Result<(std::path::PathBuf, UnixStream)> {
    let socket_path = std::env::var(SOCKET_DEFAULT_ENV)
        .context("Missing FHTC_SOCKET_PATH environment variable")?;
    let socket_path = std::path::PathBuf::from(socket_path);
    let socket = UnixStream::connect(&socket_path).context("Missing IPC socket")?;
    Ok((socket_path, socket))
}

/// Print the schema of the [`Request`] type.
pub fn print_schema() -> anyhow::Result<()> {
    let schema = schema_for!(Request);
    let schema_string = serde_json::to_string_pretty(&schema)?;
    println!("{}", schema_string);
    Ok(())
}

/// The request you send to the compositor.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Request {
    /// Request the version information of the running `fht-compositor` instance.
    Version,
    /// Request information about the connected outputs.
    Outputs,
    /// Request information about all mapped windows.
    Windows,
    /// Request information about all layer-shells.
    LayerShells,
    /// Request information about the workspace system.
    Space,
    /// Request information about a window.
    Window(usize),
    /// Request information about a workspace.
    Workspace(usize),
    /// Get a workspace id from an output name and index.
    GetWorkspace {
        /// The output name to get the workspace on. If `None`, use the focused output.
        output: Option<String>,
        /// The workspace index to get.
        index: usize,
    },
    /// Request information about the focused window.
    FocusedWindow,
    /// Request information about the focused workspace.
    FocusedWorkspace,
    /// Request the user to pick a window. On the next click, the information of the window under
    /// the pointer cursor will be sent back.
    PickWindow,
    /// Request the user to pick a layer-shell. On the next click, the information of the
    /// layer-shell under the pointer cursor will be sent back, if any.
    PickLayerShell,
    /// Request the cursor position.
    CursorPosition,
    /// Request the compositor to execute an action.
    Action(Action),
    /// Subscribe and listen to streaming response
    Subscribe(SubscribeTarget),
}

/// A subscribe request you send to the compositor.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
pub enum SubscribeTarget {
    /// Request information about all mapped windows.
    Windows,
    /// Request information about the workspace system.
    Space,
    /// Request information about a window.
    Window(usize),
    /// Request information about a workspace.
    Workspace(usize),
    /// Request information about all layer-shells.
    LayerShells,
    /// Subscribe to all request.
    ALL,
}

/// A respose from the compositor.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Response {
    /// Version information about the running `fht-compositor` instance.
    Version(String),
    /// Output information.
    Outputs(HashMap<String, Output>),
    /// All windows information.
    Windows(HashMap<usize, Window>),
    /// All layer-shells information.
    LayerShells(Vec<LayerShell>),
    /// Space information.
    Space(Space),
    /// Information about a window.
    Window(Option<Window>),
    /// Information about a workspace.
    Workspace(Option<Workspace>),
    /// The picked window by the user.
    PickedWindow(PickWindowResult),
    /// The picked layer shell by the user.
    PickedLayerShell(PickLayerShellResult),
    /// The cursor position.
    CursorPosition { x: f64, y: f64 },
    /// There was an error handling the request.
    Error(String),
    /// Noop, for requests that do not need a result/output.
    Noop,
}

/// A single output.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Output {
    /// Name of the output.
    pub name: String,
    /// The output manufacturer.
    pub make: String,
    /// The output model.
    pub model: String,
    /// Serial of the output, if known.
    pub serial: String,
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
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct OutputMode {
    /// The dimensions in physical pixels.
    pub dimensions: (u32, u32),
    /// The refresh rate in hertz.
    pub refresh: f64,
    /// Whether this mode is the preferred mode.
    pub preferred: bool,
}

#[derive(Default, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Workspace {
    /// The unique ID of this workspace. It is used to make requests regarding this specific
    /// workspace.
    pub id: usize,
    /// The [`Output`] this workspace belongs to.
    pub output: String,
    /// The [`Window`] IDs on this workspace.
    pub windows: Vec<usize>,
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Space {
    /// The [`Monitor`]s tracked by the [`Space`]
    pub monitors: HashMap<String, Monitor>,
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

/// A single layer-shell.
///
/// A layer-shell represents a component of your desktop interface. They can be for example your
/// notification popup, a bar, or some fancy widget you created.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LayerShell {
    /// The namespace of this layer-shell. It is used to define the purpose of this layer-shell.
    pub namespace: String,
    /// The [`Output::name`] this layer-shell is mapped onto.
    pub output: String,
    /// The layer this layer-shell is mapped onto.
    pub layer: Layer,
    /// The keyboard interactivity of this layer-shell.
    pub keyboard_interactivity: KeyboardInteractivity,
}

/// Types of keyboard interaction possible for a layer shell surface.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum KeyboardInteractivity {
    /// No keyboard focus is possible.
    None = 0,
    /// The layer-shell requests exclusive keyboard focus.
    Exclusive,
    /// The layer-shell requests regular keyboard focus.
    ///
    /// This tells the compositor that the layer-surface can accept keyboard input. The user can
    /// focus the layer-shell by clicking on it.
    OnDemand,
}

/// Available layers for surfaces
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Layer {
    Background = 0,
    Bottom,
    Top,
    Overlay,
}

/// An action to execute. This enum includes all possible key actions found in
/// fht-compositor-config, and additional ones that are more specific.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[cfg_attr(feature = "clap", derive(clap::Parser))]
#[cfg_attr(feature = "clap", command(subcommand_value_name = "ACTION"))]
#[cfg_attr(feature = "clap", command(subcommand_help_heading = "Actions"))]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// Exit the compositor.
    Quit,
    /// Reload compositor configuration.
    ReloadConfig,
    /// Select the next available layout in a [`Workspace`]
    SelectNextLayout {
        /// The [`Workspace::id`] layout to change. Leave as `None` for active workspace
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "workspace-id")]
        workspace_id: Option<usize>,
    },
    /// Select the next available layout in a [`Workspace`]
    SelectPreviousLayout {
        /// The [`Workspace::id`] layout to change. Leave as `None` for active workspace
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "workspace-id")]
        workspace_id: Option<usize>,
    },
    /// Set the maximized state of a window.
    MaximizeWindow {
        /// The state to set. Leave as `None` to toggle.
        #[cfg_attr(feature = "clap", arg(long))]
        state: Option<bool>,
        /// The [`Window::id`] to toggle maximized on. Leave as `None` for the focused one.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "window-id")]
        window_id: Option<usize>,
    },
    /// Set the fullscreen state of a window.
    FullscreenWindow {
        /// The state to set. Leave as `None` to toggle.
        #[cfg_attr(feature = "clap", arg(long))]
        state: Option<bool>,
        /// The [`Window::id`] to toggle fullscreen on. Leave as `None` for the focused one.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "window-id")]
        window_id: Option<usize>,
    },
    /// Set the floating state of a window.
    FloatWindow {
        /// The state to set. Leave as `None` to toggle.
        #[cfg_attr(feature = "clap", arg(long))]
        state: Option<bool>,
        /// The [`Window::id`] to toggle floating on. Leave as `None` for the focused one.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "window-id")]
        window_id: Option<usize>,
    },
    /// Center a floating window. If the window is tiled, this does nothing.
    CenterFloatingWindow {
        /// The [`Window::id`] to center on. Leave as `None` for the focused one.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "window-id")]
        window_id: Option<usize>,
    },
    /// Move a floating window. If the window is tiled, this does nothing.
    MoveFloatingWindow {
        /// The [`Window::id`] to move. Leave as `None` for the focused one.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "window-id")]
        window_id: Option<usize>,
        // The location change to apply.
        #[cfg_attr(feature = "clap", command(subcommand))]
        change: WindowLocationChange,
    },
    /// Resize a floating window. If the window is tiled, this does nothing.
    ResizeFloatingWindow {
        /// The [`Window::id`] to resize. Leave as `None` for the focused one.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "window-id")]
        window_id: Option<usize>,
        // The size change to apply.
        #[cfg_attr(feature = "clap", command(subcommand))]
        change: WindowSizeChange,
    },
    /// Focus a window. This will focus the [`Monitor`] and [`Workspace`] that hold this window,
    /// and force-change keyboard focus to it (unless there's a session lock active).
    FocusWindow {
        /// The [`Window::id`] to resize. Leave as `None` for the focused one.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "window-id")]
        window_id: usize,
    },
    /// Focus the next window in a [`Workspace`].
    FocusNextWindow {
        /// The [`Workspace::id`] layout to change. Leave as `None` for active workspace.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "workspace-id")]
        workspace_id: Option<usize>,
    },
    /// Focus the previous window in a [`Workspace`].
    FocusPreviousWindow {
        /// The [`Workspace::id`] layout to change. Leave as `None` for active workspace.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "workspace-id")]
        workspace_id: Option<usize>,
    },
    /// Swap thee currently focused window with the next window in a [`Workspace`].
    SwapWithNextWindow {
        /// Whether we should keep the currently focused window as is, or
        #[cfg_attr(feature = "clap", arg(long, default_value_t = true))]
        keep_focus: bool,
        /// The [`Workspace::id`] layout to change. Leave as `None` for active workspace.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "workspace-id")]
        workspace_id: Option<usize>,
    },
    /// Swap thee currently focused window with the previous window in a [`Workspace`].
    SwapWithPreviousWindow {
        /// Whether we should keep the currently focused window as is, or
        #[cfg_attr(feature = "clap", arg(long, default_value_t = true))]
        keep_focus: bool,
        /// The [`Workspace::id`] layout to change. Leave as `None` for active workspace.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "workspace-id")]
        workspace_id: Option<usize>,
    },
    /// Focus a given output.
    FocusOutput {
        /// The [`Output::name`] to focus.
        output: String,
    },
    /// Focus the next output relative to the current one. Output order can be retreived using
    /// [`Request::Outputs`]
    FocusNextOutput,
    /// Focus the previous output relative to the current one. Output order can be retreived using
    /// [`Request::Outputs`]
    FocusPreviousOutput,
    /// Focus a given [`Workspace`]
    FocusWorkspace {
        /// The [`Workspace`] to focus.
        #[serde(rename = "workspace-id")]
        workspace_id: usize,
    },
    /// Focus a given [`Workspace`] using an index.
    FocusWorkspaceByIndex {
        /// The [`Workspace`] to focus, r
        /// #[serde(rename = "workspace-ideferenced by index.
        workspace_idx: usize,
        /// The [`Monitor`] to change the focused [`Workspace`] on. Leave as `None` for the active
        /// one.
        output: Option<String>,
    },
    /// Focus the next workspace relative to the current one.
    FocusNextWorkspace {
        /// The [`Output`] to execute this action on. Leave as `None` for the active one.
        output: Option<String>,
    },
    /// Focus the next workspace relative to the current one.
    FocusPreviousWorkspace {
        /// The [`Output`] to execute this action on. Leave as `None` for the active one.
        output: Option<String>,
    },
    /// Close a [`Window`]
    CloseWindow {
        /// The [`Window::id`] to resize. Leave as `None` for the focused one.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "window-id")]
        window_id: Option<usize>,
        /// Whether to force-kill the window.
        #[cfg_attr(feature = "clap", arg(long, action = clap::ArgAction::Set, default_value_t = false))]
        kill: bool,
    },
    /// Change the master width factor on a [`Workspace`]
    ChangeMwfact {
        /// The [`Workspace::id`] layout to change. Leave as `None` for active workspace.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "workspace-id")]
        workspace_id: Option<usize>,
        /// The mwfact change to apply.
        #[cfg_attr(feature = "clap", command(subcommand))]
        change: MwfactChange,
    },
    /// Change the number of master clients on a [`Workspace`]
    ChangeNmaster {
        /// The [`Workspace::id`] layout to change. Leave as `None` for active workspace.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "workspace-id")]
        workspace_id: Option<usize>,
        /// The nmaster change to apply.
        #[cfg_attr(feature = "clap", command(subcommand))]
        change: NmasterChange,
    },
    /// Change [`Window::proportion`] of a window.
    ChangeWindowProportion {
        /// The [`Window::id`] to resize. Leave as `None` for the focused one.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "window-id")]
        window_id: Option<usize>,
        /// The window proportion change to apply.
        #[cfg_attr(feature = "clap", command(subcommand))]
        change: WindowProportionChange,
    },
    /// Send a [`Window`] to a [`Workspace`].
    SendWindowToWorkspace {
        /// The [`Window::id`] to resize. Leave as `None` for the focused one.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "window-id")]
        window_id: Option<usize>,
        /// The [`Workspace`] to send the window to.
        #[cfg_attr(feature = "clap", arg(long))]
        #[serde(rename = "workspace-id")]
        workspace_id: usize,
    },
}

/// A window location change.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "clap", derive(clap::Parser))]
pub enum WindowLocationChange {
    /// Add this amount to the window location.
    Change {
        #[cfg_attr(feature = "clap", arg(allow_hyphen_values = true))]
        dx: Option<i32>,
        #[cfg_attr(feature = "clap", arg(allow_hyphen_values = true))]
        dy: Option<i32>,
    },
    /// Set the window location to this value.
    Set {
        #[cfg_attr(feature = "clap", arg(allow_hyphen_values = true))]
        x: Option<i32>,
        #[cfg_attr(feature = "clap", arg(allow_hyphen_values = true))]
        y: Option<i32>,
    },
}

/// A window size change.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "clap", derive(clap::Parser))]
pub enum WindowSizeChange {
    /// Add this amount to the current window size. Clamps at (20, 20) for the minimum.
    Change {
        #[cfg_attr(feature = "clap", arg(long, allow_negative_numbers = true))]
        dx: Option<i32>,
        #[cfg_attr(feature = "clap", arg(long, allow_negative_numbers = true))]
        dy: Option<i32>,
    },
    /// Set the window size to this value.
    Set {
        #[cfg_attr(feature = "clap", arg(long, allow_negative_numbers = true))]
        x: Option<u32>,
        #[cfg_attr(feature = "clap", arg(long, allow_negative_numbers = true))]
        y: Option<u32>,
    },
}

/// A window proportion change.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[cfg_attr(feature = "clap", derive(clap::Parser))]
pub enum WindowProportionChange {
    /// Add this amount to the current window proportion.
    Change {
        #[cfg_attr(feature = "clap", arg(allow_negative_numbers = true))]
        delta: f64,
    },
    /// Set the window size to this value.
    Set {
        #[cfg_attr(feature = "clap", arg(allow_negative_numbers = true))]
        value: f64,
    },
}

/// A master width factor change.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[cfg_attr(feature = "clap", derive(clap::Parser))]
pub enum MwfactChange {
    /// Add this amount to the master width factor. Clamps inside [0.01, 0.99].
    Change {
        #[cfg_attr(feature = "clap", arg(allow_negative_numbers = true))]
        delta: f64,
    },
    /// Set the mwfact to this value.
    Set {
        #[cfg_attr(feature = "clap", arg(allow_negative_numbers = true))]
        value: f64,
    },
}

/// A number of master clients change.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[cfg_attr(feature = "clap", derive(clap::Parser))]
pub enum NmasterChange {
    /// Add this amount to the number of master clients. Clamps at min=1.
    Change {
        #[cfg_attr(feature = "clap", arg(allow_negative_numbers = true))]
        delta: i32,
    },
    /// Set the nmaster to this value. Clamps at min=1.
    Set { value: usize },
}

/// The result from picking a [`Window`].
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PickWindowResult {
    /// The ID of the picked window
    Some(usize),
    /// The user clicked somewhere outside any window.
    None,
    /// The pick request was cancelled.
    Cancelled,
}

/// The result from picking a [`LayerShell`].
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PickLayerShellResult {
    /// The information of the picked layer-shell
    Some(LayerShell),
    /// The user clicked somewhere outside any layer-shell.
    None,
    /// The pick request was cancelled.
    Cancelled,
}
