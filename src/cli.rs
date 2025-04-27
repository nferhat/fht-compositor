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

#[derive(Debug, Clone, Copy, clap::Subcommand)]
pub enum Command {
    /// Check the compositor configuration for any errors.
    CheckConfiguration,
    /// Generate shell completions for shell
    GenerateCompletions { shell: clap_complete::Shell },
    /// Execute an IPC [`Request`].
    Ipc {
        #[command(subcommand)]
        request: fht_compositor_ipc::Request,
        /// Enable JSON output formatting
        #[arg(short, long)]
        json: bool,
    },
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
