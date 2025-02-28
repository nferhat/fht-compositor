#![allow(clippy::ignored_unit_patterns)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::too_many_lines)]

// Tracing since it's used project wide for logging
#[macro_use]
extern crate tracing;

use std::error::Error;
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

    let mut state = State::new(
        &dh,
        event_loop.handle(),
        event_loop.get_signal(),
        cli.config_path,
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
    }

    for cmd in &state.fht.config.autostart {
        utils::spawn(cmd);
    }

    #[cfg(feature = "uwsm")]
    if cli.uwsm {
        // Run "uwsm finalize" in order to export environment to systemd activation
        // This will also signal that the compositor has started up and is ready to go
        match std::process::Command::new("uwsm").arg("finalize").spawn() {
            Ok(mut child) => match child.wait() {
                Ok(status) if !status.success() => {
                    warn!("uwsm finalize process exited unsuccessfully")
                }
                Err(err) => warn!(?err, "Failed to wait for uwsm child"),
                _ => (),
            },
            Err(err) => warn!(?err, "Failed to spawn uwsm finalize child"),
        }
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

#[allow(unused_imports)]
pub(crate) use profiling::{profile_function, profile_scope};
