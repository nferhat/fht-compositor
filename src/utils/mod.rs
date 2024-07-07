use std::mem::MaybeUninit;
use std::os::unix::process::CommandExt;
use std::process::Stdio;

pub mod animation;
#[cfg(feature = "dbus")]
pub mod dbus;
#[cfg(feature = "udev_backend")]
pub mod drm;
pub mod fps;
pub mod geometry;
pub mod output;
#[cfg(feature = "xdg-screencast-portal")]
pub mod pipewire;

/// Spawn a given command line using `/bin/sh`, double-forking it in order to avoid zombie
/// process even after fht-compositor dies.
#[profiling::function]
pub fn spawn(cmd: String) {
    let res = std::thread::Builder::new()
        .name("Command spawner".to_string())
        .spawn(move || {
            let mut command = std::process::Command::new("/bin/sh");
            command.args(["-c", &cmd]);
            // Disable all IO.
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());

            // Double for in order to avoid the command being a child of fht-compositor.
            // This will allow us to avoid creating zombie processes.
            //
            // This also lets us not waitpid from the child
            unsafe {
                command.pre_exec(|| {
                    match libc::fork() {
                        -1 => return Err(std::io::Error::last_os_error()),
                        0 => (),
                        _ => libc::_exit(0),
                    }

                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }

                    // Reset signal handlers.
                    let mut signal_set = MaybeUninit::uninit();
                    libc::sigemptyset(signal_set.as_mut_ptr());
                    libc::sigprocmask(
                        libc::SIG_SETMASK,
                        signal_set.as_mut_ptr(),
                        std::ptr::null_mut(),
                    );

                    Ok(())
                });
            }

            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(err) => {
                    warn!(?err, ?cmd, "Error spawning command");
                    return;
                }
            };

            match child.wait() {
                Ok(status) => {
                    if !status.success() {
                        warn!(?status, "Child didn't exit sucessfully!");
                    }
                }
                Err(err) => {
                    warn!(?err, "Failed to wait for child!");
                }
            }
        });

    if let Err(err) = res {
        warn!(?err, "Failed to create command spawner for command!");
    }
}
