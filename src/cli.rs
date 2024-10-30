use std::path::PathBuf;

#[derive(Debug, clap::Parser)]
pub struct Cli {
    /// What backend should the compositor start with?
    #[arg(short, long, value_name = "BACKEND")]
    pub backend: Option<BackendType>,
    /// The configuration path to use.
    #[arg(short, long, value_name = "PATH")]
    pub config_path: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Copy, clap::Subcommand)]
pub enum Command {
    /// Check the compositor configuration for any errors.
    CheckConfiguration,
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
