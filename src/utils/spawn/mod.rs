//! Spawning utilities
//!
//! You should always use `utils::run_command` or `utils::run`, these are low-level functions

#[cfg(not(feature = "systemd"))]
mod generic;
#[cfg(feature = "systemd")]
mod systemd;

#[cfg(not(feature = "systemd"))]
pub use generic::*;
#[cfg(feature = "systemd")]
pub use systemd::*;
