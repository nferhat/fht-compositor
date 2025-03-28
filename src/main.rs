#![allow(clippy::ignored_unit_patterns)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::too_many_lines)]

// Tracing since it's used project wide for logging
#[macro_use]
extern crate tracing;

use std::error::Error;
use std::io::Write;
use std::process::Command;
use std::str::FromStr;
use std::sync::Arc;

use clap::{CommandFactory, Parser};
use smithay::reexports::calloop::generic::{Generic, NoIoDrop};
use smithay::reexports::calloop::{EventLoop, Interest, Mode};
use smithay::reexports::wayland_server::Display;
use smithay::wayland::socket::ListeningSocketSource;
use state::State;

mod backend;
mod cli;
mod config;
mod cursor;
mod egui;
mod focus_target;
mod frame_clock;
mod handlers;
mod input;
mod ipc;
mod layer;
mod output;
#[cfg(any(feature = "xdg-screencast-portal"))]
mod portals;
mod profiling;
mod protocols;
mod renderer;
mod space;
mod state;
mod utils;
mod window;

#[cfg(feature = "profile-with-tracy-allocations")]
#[global_allocator]
static GLOBAL: tracy_client::ProfiledAllocator<std::alloc::System> =
    tracy_client::ProfiledAllocator::new(std::alloc::System, 100);

fn main() -> anyhow::Result<(), Box<dyn Error>> {
    // Do not allow the user to build a useless compositor.
    //
    // We must have at least one backend, otherwise unmatched branches will occur.
    // This also must be at the very top of the crate so that it pops ups before anything.
    #[cfg(all(
        not(feature = "udev-backend"),
        not(feature = "winit-backend"),
        not(feature = "headless-backend")
    ))]
    compile_error!("You must enable at least one backend: 'udev-backend' or 'winit-backend");

    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        // Allow fatal errors from every crate, compositor can log anything
        tracing_subscriber::EnvFilter::from_str("error,fht_compositor=info").unwrap()
    });
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(filter)
        .init();

    let cli = cli::Cli::parse();
    match cli.command {
        Some(cli::Command::CheckConfiguration) => check_configuration(cli),
        Some(cli::Command::GenerateCompletions { shell }) => {
            let mut command = cli::Cli::command();
            let name = command.get_name().to_string();
            clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
            std::process::exit(0); // we just want to generate completions, nothing much
        }
        _ => (),
    }
    // Start tracy client now since everything before is just basic setup or command handling.
    // NOTE: If enabled feature is not toggled this does nothing
    tracy_client::Client::start();

    info!(
        version = std::env!("CARGO_PKG_VERSION"),
        git_hash = std::option_env!("GIT_HASH").unwrap_or("Unknown"),
        "Starting fht-compositor."
    );

    // EventLoop + Wayland UNIX socket source so we can listen to clients
    let mut event_loop: EventLoop<State> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    let (dh, socket_name) = {
        let display: Display<State> = Display::new()?;
        let dh = display.handle();
        let listening_socket = ListeningSocketSource::new_auto()?;
        let socket_name = String::from(listening_socket.socket_name().to_string_lossy());

        loop_handle
            .insert_source(listening_socket, |client_stream, _, state| {
                // Insert the client on the wayland display.
                // + Additional data (ATM only compositor_client_state)

                let ret = state
                    .fht
                    .display_handle
                    .insert_client(client_stream, Arc::new(state.new_client_state()));
                if let Err(err) = ret {
                    warn!(?err, "Failed to add wayland client to display");
                }
            })
            .expect("Failed to init the Wayland event source!");
        info!(?socket_name, "Listening on socket");

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display: &mut NoIoDrop<Display<State>>, state| {
                    unsafe {
                        display
                            .get_mut()
                            .dispatch_clients(state)
                            .expect("Failed to display clients!");
                    }
                    Ok(smithay::reexports::calloop::PostAction::Continue)
                },
            )
            .expect("Failed to init the Wayland event source!");

        (dh, socket_name)
    };

    // NOTE: For IPC we must start it **before** creating and spawning autostart to have ready state
    // to replicate ASAP. This is needed if for example autostart/xdg-autostart has a dependency on
    // the socket being present.
    let ipc_server = ipc::start(&loop_handle, &socket_name)
        .inspect_err(|err| error!(?err, "Failed to start IPC server"))
        .ok();

    let mut state = State::new(
        &dh,
        event_loop.handle(),
        event_loop.get_signal(),
        cli.config_path,
        ipc_server,
        cli.backend,
        socket_name.clone(),
    );

    // SAFETY: We do not access these environment variables during these writes/set_var calls,
    // so the race-condition concerns should be non-existent.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &socket_name);
        std::env::set_var("XDG_CURRENT_DESKTOP", "fht-compositor");
        std::env::set_var("XDG_SESSION_TYPE", "wayland");
        std::env::set_var("MOZ_ENABLE_WAYLAND", "1");
        std::env::set_var("_JAVA_AWT_NONREPARENTING", "1");

        for (key, value) in &state.fht.config.env {
            std::env::set_var(key, value);
        }
    }

    #[cfg(any(feature = "xdg-screencast-portal"))]
    if let Some(dbus_connection) = &state.fht.dbus_connection {
        if let Err(err) = portals::start(dbus_connection, &loop_handle) {
            error!(?err, "Failed to start XDG portals")
        }
    }

    // Before starting the compositor, we export the environment to systemd and the dbus activation
    // environment, and before spawning our programs and services that rely on it.
    //
    // FIXME: More system manaagers support, I heard dinit and OpenRC got their user-services
    // implemented now. For now we only support systemd, but keep this in the back of our head
    // for the future.
    if cli.session {
        let vars = [
            "WAYLAND_DISPLAY",
            "XDG_CURRENT_DESKTOP",
            "XDG_SESSION_TYPE",
            "MOZ_ENABLE_WAYLAND",
            "_JAVA_AWT_NONREPARENTING",
        ];
        let vars_str = vars.join(" ");

        let system_manager_cmd = if cfg!(feature = "systemd") {
            format!("systemctl --user import-environment {vars_str}")
        } else {
            // No system manager integration
            String::new()
        };

        let import_cmd = format!(
            "
                {system_manager_cmd} 2>&1;
                dbus-update-activation-environment --systemd {vars_str};
            "
        );
        let rv = Command::new("/bin/sh").args(["-c", &import_cmd]).spawn();
        match rv {
            Ok(mut child) => match child.wait() {
                Ok(status) if !status.success() => {
                    warn!(?status, "Import environment variables command exited")
                }
                Err(err) => {
                    warn!(?err, "Import environment variable command failed with")
                }
                _ => (), // success continue
            },
            Err(err) => {
                warn!(
                    ?err,
                    "Failed to spawn shell for importing environment variables"
                )
            }
        }

        #[cfg(feature = "systemd")]
        {
            use std::env;
            use std::os::fd::FromRawFd;

            // Notify systemd about ready status
            if let Err(err) = sd_notify::notify(true, &[sd_notify::NotifyState::Ready]) {
                warn!(
                    ?err,
                    "Failed to notify systemd about ready status through sd-notify"
                );
            }
            // Also support NOTIFY_FD, in case we are not using socket-based communication with
            // systemd
            let notify_fd_result = (|| -> anyhow::Result<()> {
                let fd = match env::var("NOTIFY_FD") {
                    Ok(value) => value.parse()?,
                    // Don't do anything if it's not advertised.
                    Err(env::VarError::NotPresent) => return Ok(()),
                    Err(err) => anyhow::bail!(err),
                };
                let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
                file.write_all(b"READY=1\n")?;
                Ok(())
            })();

            if let Err(err) = notify_fd_result {
                warn!(
                    ?err,
                    "Failed to notify systemd about ready status through NOTIFY_FD"
                )
            }
        }
    }

    // We also spawn programs before running the event loop, but after setting up the environment
    // and notifying the system manager about ready status.
    //
    // Since we are already listening on a socket, so they can connect to the compositor, and will
    // be ready (hopefully) on the first rendered frame.
    for cmd in &state.fht.config.autostart {
        utils::spawn(cmd);
    }

    event_loop
        .run(None, &mut state, |state| {
            if state.fht.stop {
                state.fht.loop_signal.stop();
                state.fht.loop_signal.wakeup();
                return;
            }

            state.dispatch().unwrap();
        })
        .expect("Failed to run the eventloop!");

    // Stop the socket and remove it
    if let Some(ipc_server) = state.fht.ipc_server.take() {
        ipc_server.close(&loop_handle);
    }

    std::mem::drop(event_loop);
    std::mem::drop(state);

    info!("Shutting down! Goodbye~");

    Ok(())
}

fn check_configuration(cli: cli::Cli) -> ! {
    match fht_compositor_config::load(cli.config_path) {
        Ok(_) => {
            info!("There are no issues with your configuration");
            std::process::exit(0)
        }
        Err(err) => match err {
            fht_compositor_config::Error::IO(err) => {
                error!(?err, "Failed to load your configuration");
                std::process::exit(1)
            }
            fht_compositor_config::Error::Parse(err) => {
                // toml error has a pretty formatter that is good enough for this.
                print!("\n{}", err);
                std::process::exit(1)
            }
        },
    }
}

#[allow(unused_imports)]
pub(crate) use profiling::{profile_function, profile_scope};
