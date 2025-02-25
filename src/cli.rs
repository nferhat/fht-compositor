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
    /// Whether to run `uwsm` to finalize the compositor environment.
    #[arg(long)]
    #[cfg(feature = "uwsm")]
    pub uwsm: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Copy, clap::Subcommand)]
pub enum Command {
    /// Check the compositor configuration for any errors.
    CheckConfiguration,
    /// Generate shell completions for shell
    GenerateCompletions { shell: clap_complete::Shell },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum BackendType {
    #[cfg(feature = "winit-backend")]
    /// Use the Winit backend, inside an Winit window.
    Winit,
    #[cfg(feature = "udev-backend")]
    /// Use the Udev backend, using a libseat session.
    Udev,
}

fn get_version_string() -> String {
    format!(
        "{} ({})",
        std::env!("CARGO_PKG_VERSION"),
        std::option_env!("GIT_HASH").unwrap_or("unknown git revision")
    )
}
