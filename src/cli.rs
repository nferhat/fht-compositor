use std::path::PathBuf;

use clap::builder::styling::{AnsiColor, Effects};
use clap::builder::Styles;

pub const CLAP_STYLING: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default())
    .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
    .valid(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .invalid(AnsiColor::Yellow.on_default().effects(Effects::BOLD));

#[derive(Debug, clap::Parser)]
#[command(author, version = get_version_string(), about, long_about = None, styles = CLAP_STYLING)]
pub struct Cli {
    /// What backend should the compositor start with?
    #[arg(short, long, value_name = "BACKEND")]
    pub backend: Option<BackendType>,
    /// The configuration path to use.
    #[arg(short, long, value_name = "PATH")]
    pub config_path: Option<PathBuf>,
    /// Whether to run fht-compositor as a session
    #[arg(long)]
    pub session: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, clap::Subcommand)]
pub enum Command {
    /// Check the compositor configuration for any errors.
    CheckConfiguration,
    /// Generate shell completions for shell
    GenerateCompletions { shell: clap_complete::Shell },
    /// Execute an IPC [`Request`].
    Ipc {
        #[command(subcommand)]
        request: Request,
        /// Enable JSON output formatting
        #[arg(short, long)]
        json: bool,
        /// Subscribe and listen to streaming response
        #[arg(short, long)]
        subscribe: bool,
    },
}

/// A request you send to the compositor.
#[derive(Debug, Clone, PartialEq, clap::Subcommand)]
pub enum Request {
    /// Request the version information of the running `fht-compositor` instance.
    Version,
    /// Request information about the connected outputs.
    Outputs,
    /// Request information about all mapped windows.
    Windows,
    /// Request information about the workspace system.
    Space,
    /// Request information about a window.
    Window {
        #[arg(long)]
        id: usize,
    },
    /// Request information about a workspace.
    Workspace {
        #[arg(long)]
        id: usize,
    },
    /// Get a workspace from an output name and index.
    GetWorkspace {
        /// The output name to get the workspace on. If not provided, use the focused output.
        #[arg(long)]
        output: Option<String>,
        /// The workspace index to get.
        #[arg(long)]
        index: usize,
    },
    /// Request information about the focused window.
    FocusedWindow,
    /// Request information about the focused workspace.
    FocusedWorkspace,
    /// Request information about all layer-shells.
    LayerShells,
    /// Request the user to pick a window. On the next click, the information of the window under
    /// the pointer cursor will be sent back.
    PickWindow,
    /// Request the user to pick a layer-shell. On the next click, the information of the
    /// layer-shell under the pointer cursor will be sent back, if any.
    PickLayerShell,
    /// Request the compositor to execute an action.
    Action {
        #[command(subcommand)]
        action: fht_compositor_ipc::Action,
    },
    /// Print the JSON schema for the IPC [`Request`](fht_compositor_ipc::Request) type. You can
    /// feed this schema into generators to integrate with other languages.
    PrintSchema,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum BackendType {
    #[cfg(feature = "winit-backend")]
    /// Use the Winit backend, inside an Winit window.
    Winit,
    #[cfg(feature = "udev-backend")]
    /// Use the Udev backend, using a libseat session.
    Udev,
    #[cfg(feature = "headless-backend")]
    /// Use the headless backend, only meant for testing.
    Headless,
}

pub fn get_version_string() -> String {
    let major = env!("CARGO_PKG_VERSION_MAJOR");
    let minor = env!("CARGO_PKG_VERSION_MINOR");
    let patch = env!("CARGO_PKG_VERSION_PATCH");
    let commit = option_env!("GIT_HASH").unwrap_or("unknown");

    // Since cargo forces us to "follow" semantic versionning, we must work around it.
    // Release 25.03 will be marked as 25.3.0 in Cargo.toml
    if patch == "0" {
        format!("{major}.{minor:0>2} ({commit})")
    } else {
        format!("{major}.{minor:0>2}.{patch} ({commit})")
    }
}
