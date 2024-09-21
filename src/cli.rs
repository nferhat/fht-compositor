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
    #[cfg(feature = "x11_backend")]
    /// Use the X11 backend, inside an X11 window.
    X11,
    #[cfg(feature = "udev_backend")]
    /// Use the Udev backend, using a libseat session.
    Udev,
}
