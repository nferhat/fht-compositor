use smithay::delegate_xdg_dialog;
use smithay::utils::Rectangle;
use smithay::wayland::shell::xdg::dialog::XdgDialogHandler;
use smithay::wayland::shell::xdg::ToplevelSurface;

use crate::output::OutputExt;
use crate::state::State;
use crate::utils::RectCenterExt;

impl XdgDialogHandler for State {
    fn modal_changed(&mut self, toplevel: ToplevelSurface, is_modal: bool) {
        let Some(workspace) = self
            .fht
            .space
            .workspace_mut_for_window_surface(toplevel.wl_surface())
        else {
            warn!("Received modal_changed for unmapped toplevel");
            return;
        };
        let output_rect = Rectangle::from_size(workspace.output().geometry().size);

        if !is_modal {
            // I mean, we kinda don't care if its not.
            return;
        }

        let tile = workspace
            .tiles_mut()
            .find(|tile| *tile.window().toplevel() == toplevel)
            .unwrap();
        tile.window().request_tiled(false);
        // Ask the toplevel to set its own size according to whatever it likes.
        // For modals/dialogs it should set whatever needed size.
        tile.window().reset_size();
        tile.window().send_configure();

        // Now center the tile.
        let tile_size = tile.size();
        let loc = output_rect.center() - tile_size.to_f64().downscale(2.0).to_i32_round();
        tile.set_location(loc, !self.fht.config.animations.disable);

        // Now re-arrange in case the modal window was tiled.
        workspace.arrange_tiles(!self.fht.config.animations.disable);
    }
}

delegate_xdg_dialog!(State);
