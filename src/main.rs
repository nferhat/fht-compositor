#![allow(clippy::ignored_unit_patterns)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::too_many_lines)]

// Tracing since it's used project wide for logging
#[macro_use]
extern crate tracing;

use std::error::Error;
use std::fmt::Write as _;
use std::io::Write as _;
use std::os::fd::FromRawFd as _;
use std::str::FromStr;
use std::sync::Arc;
use std::{env, fs};

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
    #[cfg(all(not(feature = "udev-backend"), not(feature = "winit-backend")))]
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

    #[cfg(any(feature = "xdg-screencast-portal"))]
    if let Err(err) = portals::start(&loop_handle) {
        error!(?err, "Failed to start XDG portals")
    }

    let session = cli.session;
    let mut state = State::new(
        &dh,
        event_loop.handle(),
        event_loop.get_signal(),
        cli,
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
    }

    if session {
        // Session setup:
        // - Import the environment values we set into systemd/dbus
        // - Notify systemd service that we are ready.
        // - Handle NOTIFY_FD if any.
        //
        // This is needed otherwise systemd will kill our service (IE. the compositor session) since
        // it times out at 1 minute 30 seconds for ready notification.
        import_environment();

        if let Err(err) = sd_notify::notify(true, &[sd_notify::NotifyState::Ready]) {
            warn!("Error notifying systemd: {err:?}")
        }

        if let Err(err) = notify_fd() {
            warn!("Error notifying fd: {err:?}")
        }
    }

    for cmd in &state.fht.config.autostart {
        utils::spawn(cmd.clone());
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

fn import_environment() {
    let variables = ["WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE"].join(" ");

    let mut init_system_import = String::new();
    write!(
        init_system_import,
        "systemctl --user import-environment {variables};"
    )
    .unwrap();

    let rv = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            &format!(
                "{init_system_import}\
                 hash dbus-update-activation-environment 2>/dev/null && \
                 dbus-update-activation-environment {variables}"
            ),
        ])
        .spawn();
    // Wait for the import process to complete, otherwise services will start too fast without
    // environment variables available.
    match rv {
        Ok(mut child) => match child.wait() {
            Ok(status) => {
                if !status.success() {
                    warn!("import environment shell exited with {status}");
                }
            }
            Err(err) => {
                warn!("error waiting for import environment shell: {err:?}");
            }
        },
        Err(err) => {
            warn!("error spawning shell to import environment: {err:?}");
        }
    }
}

fn notify_fd() -> anyhow::Result<()> {
    let fd = match env::var("NOTIFY_FD") {
        Ok(notify_fd) => notify_fd.parse()?,
        Err(env::VarError::NotPresent) => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    env::remove_var("NOTIFY_FD");
    let mut notif = unsafe { fs::File::from_raw_fd(fd) };
    notif.write_all(b"READY=1\n")?;
    Ok(())
}

#[allow(unused_imports)]
pub(crate) use profiling::{profile_function, profile_scope};
