use smithay::{
    delegate_xdg_decoration,
    reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
    wayland::{
        compositor::with_states,
        shell::xdg::{decoration::XdgDecorationHandler, ToplevelSurface, XdgToplevelSurfaceData},
    },
};

use crate::{config::CONFIG, state::State};

// NOTE: Based on CONFIG.decoration.allow_csd, this will only (and forcefully) set either server
// side deco or client side deco

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // Set the default to client side
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(if CONFIG.decoration.allow_csd {
                DecorationMode::ClientSide
            } else {
                // when we dont allow CSD nothing gets drawn
                DecorationMode::ServerSide
            });
        });
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(if CONFIG.decoration.allow_csd {
                DecorationMode::ClientSide
            } else {
                DecorationMode::ServerSide
            });
        });

        let initial_configure_sent = with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });
        if initial_configure_sent {
            toplevel.send_pending_configure();
        }
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(if CONFIG.decoration.allow_csd {
                DecorationMode::ClientSide
            } else {
                DecorationMode::ServerSide
            });
        });

        let initial_configure_sent = with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });
        if initial_configure_sent {
            toplevel.send_pending_configure();
        }
    }
}

delegate_xdg_decoration!(State);
