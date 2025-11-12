//! SystemD spawning utilities.
//!
//! This module is mostly to handle properly spawning with systemd in mind, following the
//! guidelines of this article: <https://systemd.io/DESKTOP_ENVIRONMENTS>. In short, when spawning
//! applications, we create a transient scope for them and put their PID inside that scope.
//!
//! A scope is basically us spawning the process then telling systemd about it, and the fact its
//! separate from the parent process (so separate from the `fht-compositor.service` unit)
//!
//! Doing this allows systemd to group resource allocation, avoid OOM killing us, assign different
//! process priorities depending on the slice, etc...

use std::ffi::OsStr;
use std::fmt::Write;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use smithay::reexports::rustix;
use smithay::reexports::rustix::io::retry_on_intr;
use zbus::blocking::Connection;
use zvariant::{OwnedObjectPath, Value};

pub fn do_spawn(name: &OsStr, mut process: Command) -> Option<Child> {
    // In order to start the transient unit, we must get the actual process PID, since its
    // different processes, using a pipe is required instead of some channel.
    let (pipe_pid_read, pipe_pid_write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
        .map_err(|err| {
            warn!("error creating a pipe to transfer child PID: {err:?}");
        })
        .ok()
        .unzip();

    unsafe {
        let mut pipe_pid_read_fd = pipe_pid_read.as_ref().map(|fd| fd.as_raw_fd());
        let mut pipe_pid_write_fd = pipe_pid_write.as_ref().map(|fd| fd.as_raw_fd());
        // Double-forking usefulness is two-fold, first this avoids us to have to waitpid the
        // child, and this truly detaches the process from any TTY/terminal, due to process' session
        // ID changing.
        process.pre_exec(move || {
            if let Some(fd) = pipe_pid_read_fd.take() {
                libc::close(fd);
            }

            let pipe_pid_write = pipe_pid_write_fd.take().map(|fd| OwnedFd::from_raw_fd(fd));

            match libc::fork() {
                -1 => return Err(io::Error::last_os_error()),
                0 => (),
                // This is actually the grandchild, not the direct child! (due to the double-fork)
                grandchild_pid => {
                    if let Some(pipe) = pipe_pid_write {
                        let _ = write_all(pipe, &grandchild_pid.to_ne_bytes());
                    }

                    // And then close the grandchild, no need for it anymore.
                    libc::_exit(0)
                }
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
            warn!(?err, "Failed to spawn {process:?}");
            return None;
        }
    };

    drop(pipe_pid_write);
    if let Some(pipe) = pipe_pid_read {
        let mut buf = [0; 4];
        match read_all(pipe, &mut buf) {
            Ok(()) => {
                // We received the PID from the grandchild, inform systemd about it
                // and try to create a transient scope.
                let pid = i32::from_ne_bytes(buf);
                if let Err(err) = create_systemd_transient_scope(name, pid as u32) {
                    trace!(
                        ?err,
                        ?pid,
                        "Failed to create systemd transient scope for child"
                    );
                }
            }
            Err(err) => {
                warn!(?err, "Error reading child PID");
            }
        }
    }

    Some(child)
}

fn create_systemd_transient_scope(name: &OsStr, pid: u32) -> anyhow::Result<()> {
    crate::profile_function!();

    if !crate::RUNNING_AS_SYSTEMD_SERVICE.load(std::sync::atomic::Ordering::SeqCst) {
        // Not running under the .service file, no need to do handling.
        return Ok(());
    }

    let name = Path::new(name).file_name().unwrap_or(name);

    // The scope name should be something like `app-[<LAUNCHER>-]-<APPID>-<RANDOM>.scope`, where
    // launcher is left empty here, RANDOM is a bunch of randomly generated characters (to support
    // running multiple instances of the same application)
    let mut scope_name = String::from("app-");
    // Push the APPID
    // Escaping here is not obligatory but preferred.
    for &c in name.as_bytes() {
        if c.is_ascii_alphanumeric() || matches!(c, b':' | b'_' | b'.') {
            scope_name.push(char::from(c));
        } else {
            let _ = write!(scope_name, "\\x{c:02x}");
        }
    }
    // And finally the RANDOM part.
    let random = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() / 10000;
    let _ = write!(scope_name, "{random}.scope");

    // Ask systemd to start a transient scope.
    static CONNECTION: OnceLock<zbus::Result<Connection>> = OnceLock::new();
    let conn = CONNECTION
        .get_or_init(Connection::session)
        .clone()
        .context("error connecting to session bus")?;

    let proxy = zbus::blocking::Proxy::new(
        &conn,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .context("error creating a Proxy")?;

    let signals = proxy
        .receive_signal("JobRemoved")
        .context("error creating a signal iterator")?;

    // Reference on the D-Bus interface
    // <https://www.man7.org/linux/man-pages/man5/org.freedesktop.systemd1.5.html>
    let pids: &[_] = &[pid];
    let properties: &[_] = &[
        ("PIDs", Value::new(pids)),
        ("CollectMode", Value::new("inactive-or-failed")),
    ];
    // This field is currently unused and is asked to be an a(as{v})
    let aux: &[(&str, &[(&str, Value)])] = &[];

    let job: OwnedObjectPath = proxy
        .call("StartTransientUnit", &(scope_name, "fail", properties, aux))
        .context("error calling StartTransientUnit")?;

    trace!("waiting for JobRemoved");
    for message in signals {
        let body = message.body();
        let body: (u32, OwnedObjectPath, &str, &str) =
            body.deserialize().context("error parsing signal")?;

        // Keep waiting for JobRemoved until we see our unit.
        if body.1 == job {
            break;
        }
    }

    Ok(())
}

fn write_all(fd: impl AsFd, buf: &[u8]) -> rustix::io::Result<()> {
    let mut written = 0;
    loop {
        let n = retry_on_intr(|| rustix::io::write(&fd, &buf[written..]))?;
        if n == 0 {
            return Err(rustix::io::Errno::CANCELED);
        }

        written += n;
        if written == buf.len() {
            return Ok(());
        }
    }
}

fn read_all(fd: impl AsFd, buf: &mut [u8]) -> rustix::io::Result<()> {
    let mut start = 0;
    loop {
        let n = retry_on_intr(|| rustix::io::read(&fd, &mut buf[start..]))?;
        if n == 0 {
            return Err(rustix::io::Errno::CANCELED);
        }

        start += n;
        if start == buf.len() {
            return Ok(());
        }
    }
}
