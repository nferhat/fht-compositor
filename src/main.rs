#![allow(clippy::ignored_unit_patterns)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::too_many_lines)]

// Tracing since it's used project wide for logging
#[macro_use]
extern crate tracing;

use std::error::Error;
use std::str::FromStr;
use std::sync::Arc;

use clap::Parser;
use smithay::reexports::calloop::generic::{Generic, NoIoDrop};
use smithay::reexports::calloop::{EventLoop, Interest, Mode};
use smithay::reexports::wayland_server::Display;
use smithay::wayland::socket::ListeningSocketSource;
use state::State;

mod backend;
mod cli;
mod config;
mod egui;
mod frame_clock;
mod handlers;
mod input;
mod output;
#[cfg(any(feature = "xdg-screencast-portal"))]
mod portals;
mod protocols;
mod renderer;
mod shell;
mod space;
mod state;
mod utils;
mod window;

fn main() -> anyhow::Result<(), Box<dyn Error>> {
    // Logging.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        // Allow fatal errors from every crate, compositor can log anything
        tracing_subscriber::EnvFilter::from_str("error,fht_compositor=info").unwrap()
    });
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(filter)
        .init();

    let cli = cli::Cli::parse();
    if let Some(cli::Command::CheckConfiguration) = cli.command {
        check_configuration(cli);
    }

    info!(
        version = std::env!("CARGO_PKG_VERSION"),
        git_hash = std::option_env!("GIT_HASH").unwrap_or("Unknown"),
        "Starting fht-compositor."
    );

    #[cfg(feature = "profile-with-tracy")]
    {
        profiling::register_thread!("Main Thread");
        profiling::tracy_client::Client::start();
    }

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
            info!("There's no issues with your configuration");
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
