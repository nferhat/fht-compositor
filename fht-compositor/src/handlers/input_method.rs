use smithay::delegate_input_method_manager;
use smithay::desktop::space::SpaceElement;
use smithay::desktop::{PopupKind, PopupManager};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Rectangle;
use smithay::wayland::input_method::{InputMethodHandler, PopupSurface};

use crate::state::State;

impl InputMethodHandler for State {
    fn new_popup(&mut self, surface: PopupSurface) {
        if let Err(err) = self.fht.popups.track_popup(PopupKind::from(surface)) {
            warn!("Failed to track popup: {}", err);
        }
    }

    fn dismiss_popup(&mut self, surface: PopupSurface) {
        if let Some(parent) = surface.get_parent().map(|parent| parent.surface.clone()) {
            let _ = PopupManager::dismiss_popup(&parent, &PopupKind::from(surface));
        }
    }

    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, smithay::utils::Logical> {
        self.fht
            .find_window(parent)
            .map(SpaceElement::geometry)
            .unwrap_or_default()
    }
}

delegate_input_method_manager!(State);
