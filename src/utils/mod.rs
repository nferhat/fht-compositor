use std::ffi::{OsStr, OsString};
use std::process::{Command, Stdio};
use std::time::Duration;

mod spawn;

use smithay::reexports::rustix;
use smithay::reexports::wayland_server::backend::Credentials;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};
use smithay::utils::{Coordinate, Point, Rectangle};

#[cfg(feature = "xdg-screencast-portal")]
pub mod pipewire;

pub fn get_monotonic_time() -> Duration {
    // This does the same job as a Clock<Monotonic> provided by smithay.
    // I do not understand why they decided to put on an abstraction
    //
    // We also do not use the Time<Monotonic> structure provided by smithay since its really
    // annoying to work with (addition, difference, etc...)
    let timespec = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    Duration::new(timespec.tv_sec as u64, timespec.tv_nsec as u32)
}

pub fn spawn_args<S>(command: Vec<S>)
where
    S: AsRef<OsStr> + Send + 'static,
{
    crate::profile_function!();
    if command.is_empty() {
        return;
    }

    let res = std::thread::Builder::new()
        .name("Command spawner".to_string())
        .spawn(move || {
            let (command, args) = command.split_first().unwrap();
            let mut process = Command::new(command);
            process
                .args(args)
                // Disable all IO.
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());

            // FIXME: We don't sync up the environment with the one in the configuration.
            // On each config reload, we should sync up with some static variable and use that
            // instead.
            if let Some(mut child) = spawn::do_spawn(command.as_ref(), process) {
                match child.wait() {
                    Ok(status) => {
                        if !status.success() {
                            warn!(?status, "Child did not exit successfully");
                        }
                    }
                    Err(err) => {
                        warn!(?err, "Error waiting for child");
                    }
                }
            }
        });

    if let Err(err) = res {
        warn!(?err, "Failed to create command spawner for command")
    }
}

pub fn spawn(cmdline: impl Into<OsString>) {
    crate::profile_function!();

    // To spawn a commandline, just evaluate it through sh. There are several advantages of doing
    // this instead of using a command+arguments. Notably, this allows us to take advantage of
    // shell expantions, like $ENV_VARIABLES.
    let command = vec![
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        cmdline.into(),
    ];

    spawn_args(command);
}

pub fn get_credentials_for_surface(surface: &WlSurface) -> Option<Credentials> {
    let handle = surface.handle().upgrade()?;
    let dh = DisplayHandle::from(handle);
    let client = dh.get_client(surface.id()).ok()?;
    client.get_credentials(&dh).ok()
}

pub trait RectCenterExt<C: Coordinate, Kind> {
    fn center(self) -> Point<C, Kind>;
}

impl<Kind> RectCenterExt<i32, Kind> for Rectangle<i32, Kind> {
    fn center(self) -> Point<i32, Kind> {
        self.loc + self.size.downscale(2).to_point()
    }
}

impl<Kind> RectCenterExt<f64, Kind> for Rectangle<f64, Kind> {
    fn center(self) -> Point<f64, Kind> {
        self.loc + self.size.downscale(2.0).to_point()
    }
}
