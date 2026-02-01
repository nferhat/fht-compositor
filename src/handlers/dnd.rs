use smithay::desktop::find_popup_root_surface;
use smithay::input::dnd::{self, DnDGrab, DndGrabHandler};
use smithay::input::pointer::Focus;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::WaylandDndGrabHandler;

use crate::output::OutputExt;
use crate::state::State;

impl WaylandDndGrabHandler for State {
    fn dnd_requested<S: smithay::input::dnd::Source>(
        &mut self,
        source: S,
        icon: Option<WlSurface>,
        seat: smithay::input::Seat<Self>,
        serial: smithay::utils::Serial,
        type_: smithay::input::dnd::GrabType,
    ) {
        self.fht.dnd_icon = icon;

        match type_ {
            dnd::GrabType::Pointer => {
                let pointer = seat.get_pointer().unwrap();
                let start_data = pointer.grab_start_data().unwrap();
                let grab = DnDGrab::new_pointer(&self.fht.display_handle, start_data, source, seat);
                pointer.set_grab(self, grab, serial, Focus::Keep);
            }
            dnd::GrabType::Touch => _ = (source, serial),
        }

        // FIXME: more granular
        self.fht.queue_redraw_all();
    }
}

impl DndGrabHandler for State {
    fn dropped(
        &mut self,
        target: Option<dnd::DndTarget<'_, Self>>,
        validated: bool,
        _seat: smithay::input::Seat<Self>,
        location: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) {
        // Focus the activated output.
        let mut focus_output = false;
        if let Some(target) = validated.then_some(target).flatten() {
            // SAFETY: We only do Pointer grabs for now
            let target_surface = target.into_inner().wl_surface().unwrap();
            let root = self
                .fht
                .root_surfaces
                .get(&*target_surface)
                .cloned()
                .or_else(|| {
                    // Popups.
                    self.fht
                        .popups
                        .find_popup(&*target_surface)
                        .and_then(|p| find_popup_root_surface(&p).ok())
                })
                .unwrap_or_else(|| target_surface.into_owned());
            if let Some(window) = self.fht.space.find_window(&root) {
                self.fht.space.activate_window(&window, true);
                self.fht.focused_on_demand_layer_shell = None;
                focus_output = true;
            }
        }

        if focus_output {
            let output = self
                .fht
                .space
                .outputs()
                .find(|o| o.geometry().to_f64().contains(location))
                .cloned();

            if let Some(output) = output {
                self.fht.space.set_active_output(&output);
            }
        }
    }
}
