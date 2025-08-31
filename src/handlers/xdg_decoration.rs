use smithay::{
    delegate_xdg_decoration,
    reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
    wayland::shell::xdg::{decoration::XdgDecorationHandler, ToplevelSurface},
};

use crate::state::State;

// NOTE: Based on CONFIG.decoration.allow_csd, this will only (and forcefully) set either server
// side deco or client side deco
// This file only exists to adversite the protocol to clients that support it. The decoration mode
// is set when mapping windows (see [`Fht::prepare_map_window`](../shell/mod.rs))

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, _toplevel: ToplevelSurface) {}

    fn request_mode(&mut self, _toplevel: ToplevelSurface, _mode: DecorationMode) {}

    fn unset_mode(&mut self, _toplevel: ToplevelSurface) {}
}

delegate_xdg_decoration!(State);
