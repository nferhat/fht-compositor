//! Generic process spawning utilities.

use std::ffi::OsStr;
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

pub fn do_spawn(name: &OsStr, mut process: Command) -> Option<Child> {
    // Double for in order to avoid the command being a child of fht-compositor.
    // This will allow us to avoid creating zombie processes.
    // This also lets us not waitpid the child
    unsafe {
        process.pre_exec(|| {
            match libc::fork() {
                -1 => return Err(io::Error::last_os_error()),
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

    let child = match process.spawn() {
        Ok(child) => child,
        Err(err) => {
            warn!(?err, ?name, "Error spawning command");
            return None;
        }
    };

    Some(child)
}
