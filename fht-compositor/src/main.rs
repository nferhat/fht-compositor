// rust 1.77.0
#![feature(lazy_cell)]
#![feature(sync_unsafe_cell)]
#![feature(option_take_if)]
#![feature(let_chains)]
#![feature(duration_millis_float)]
// lints
#![allow(clippy::ignored_unit_patterns)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::too_many_lines)]

// Tracing since it's used project wide for logging
#[macro_use]
extern crate tracing;

use std::error::Error;
use std::str::FromStr;
use std::sync::Arc;

use fht_config::Config;
use smithay::reexports::calloop::generic::{Generic, NoIoDrop};
use smithay::reexports::calloop::{EventLoop, Interest, Mode};
use smithay::reexports::wayland_server::Display;
use smithay::wayland::socket::ListeningSocketSource;
use state::State;

use crate::config::{CompositorConfig, CONFIG};

mod backend;
mod config;
mod egui;
mod handlers;
mod input;
mod ipc;
mod portals;
mod protocols;
mod renderer;
mod shell;
mod state;
mod utils;

fn main() -> anyhow::Result<(), Box<dyn Error>> {
    // Logging.
    // color_eyre for pretty panics
    color_eyre::install()?;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::from_str(if cfg!(debug) || cfg!(debug_assertions) {
            "debug"
        } else {
            "error,warn,fht_compositor=info"
        })
        .unwrap()
    });
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(filter)
        // .without_time()
        .init();

    info!(
        version = std::env!("CARGO_PKG_VERSION"),
        git_hash = std::option_env!("GIT_HASH").unwrap_or("Unknown"),
        "Starting fht-compositor."
    );

    #[cfg(feature = "profile-with-puffin")]
    let _puffin_server = {
        profiling::register_thread!("Main Thread");
        let server_addr = format!("0.0.0.0:{}", puffin_http::DEFAULT_PORT);
        let _puffin_server =
            puffin_http::Server::new(&server_addr).expect("Failed to start profiler!");
        profiling::puffin::set_scopes_on(true);

        info!(?server_addr, "Puffin profiler listening.");
        _puffin_server
    };

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
                    warn!(?err, "Failed to add wayland client to display!");
                }
            })
            .expect("Failed to init the Wayland event source!");
        info!(?socket_name, "Listening on socket.");

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

    if let Err(err) = config::init_config_file_watcher(&loop_handle) {
        error!(?err, "Failed to start config file watcher!");
    }
    ipc::start(&loop_handle).expect("Failed to start IPC connection!");
    portals::start(&loop_handle).expect("Failed to setup portal!");

    // Load the configuration before the state, since the state itself uses the config.
    let mut last_config_error = None;
    match CompositorConfig::load() {
        Ok(config) => {
            info!("Loaded config.");
            // The config is always initialized to the default values as a failsafe.
            // Update them now
            CONFIG.set(config);
        }
        Err(err) => {
            error!(?err, "Failed to load config!");
            last_config_error = Some(anyhow::anyhow!(err));
            CONFIG.set(CompositorConfig::default())
        }
    }

    let mut state = State::new(
        &dh,
        event_loop.handle(),
        event_loop.get_signal(),
        socket_name.clone(),
    );
    state.fht.last_config_error = last_config_error;

    std::env::set_var("WAYLAND_DISPLAY", &socket_name);
    std::env::set_var("XDG_CURRENT_DESKTOP", "fht-compositor");
    std::env::set_var("XDG_SESSION_TYPE", "wayland");
    std::env::set_var("MOZ_ENABLE_WAYLAND", "1");
    std::env::set_var("_JAVA_AWT_NONREPARENTING", "1");

    for cmd in &CONFIG.autostart {
        utils::spawn(cmd.clone());
    }

    event_loop
        .run(None, &mut state, |state| {
            if state.fht.stop.load(std::sync::atomic::Ordering::SeqCst) {
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
