use std::cell::RefCell;

use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::{Id, Kind};
use smithay::desktop::{PopupManager, Window};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
use smithay::utils::{Logical, Physical, Point, Scale, Serial, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use super::workspaces::tile::WorkspaceElement;
use crate::renderer::FhtRenderer;

struct WindowOffscreenId(RefCell<Option<Id>>);

impl WorkspaceElement for Window {
    fn send_pending_configure(&self) -> Option<Serial> {
        self.toplevel().unwrap().send_pending_configure()
    }

    fn set_size(&self, new_size: Size<i32, Logical>) {
        self.toplevel().unwrap().with_pending_state(|state| {
            state.size = Some(new_size);
        });
    }

    fn size(&self) -> Size<i32, Logical> {
        self.geometry().size
    }

    fn set_fullscreen(&self, fullscreen: bool) {
        self.toplevel().unwrap().with_pending_state(|state| {
            if fullscreen {
                state.states.set(State::Fullscreen)
            } else {
                state.states.unset(State::Fullscreen)
            }
        });
    }

    fn set_fullscreen_output(
        &self,
        output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        self.toplevel()
            .unwrap()
            .with_pending_state(|state| state.fullscreen_output = output);
    }

    fn fullscreen(&self) -> bool {
        self.toplevel()
            .unwrap()
            .with_pending_state(|state| state.states.contains(State::Fullscreen))
    }

    fn fullscreen_output(
        &self,
    ) -> Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput> {
        self.toplevel()
            .unwrap()
            .with_pending_state(|state| state.fullscreen_output.clone())
    }

    fn set_maximized(&self, maximize: bool) {
        self.toplevel().unwrap().with_pending_state(|state| {
            if maximize {
                state.states.set(State::Maximized)
            } else {
                state.states.unset(State::Maximized)
            }
        });
    }

    fn maximized(&self) -> bool {
        self.toplevel()
            .unwrap()
            .with_pending_state(|state| state.states.contains(State::Maximized))
    }

    fn set_bounds(&self, bounds: Option<Size<i32, Logical>>) {
        self.toplevel()
            .unwrap()
            .with_pending_state(|state| state.bounds = bounds);
    }

    fn bounds(&self) -> Option<Size<i32, Logical>> {
        self.toplevel()
            .unwrap()
            .with_pending_state(|state| state.bounds)
    }

    fn set_activated(&self, activated: bool) {
        self.toplevel().unwrap().with_pending_state(|state| {
            if activated {
                state.states.set(State::Activated)
            } else {
                state.states.unset(State::Activated)
            }
        });
    }

    fn activated(&self) -> bool {
        self.toplevel()
            .unwrap()
            .with_pending_state(|state| state.states.contains(State::Activated))
    }

    fn title(&self) -> String {
        with_states(self.wl_surface().as_ref().unwrap(), |states| {
            let data = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            data.title.clone().unwrap_or_default()
        })
    }

    fn app_id(&self) -> String {
        with_states(self.wl_surface().as_ref().unwrap(), |states| {
            let data = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            data.app_id.clone().unwrap_or_default()
        })
    }

    fn render_surface_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        mut location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<WaylandSurfaceRenderElement<R>> {
        let Some(surface) = self.wl_surface() else {
            return vec![];
        };

        location -= self.geometry().loc.to_physical_precise_round(scale);
        render_elements_from_surface_tree(
            renderer,
            &surface,
            location,
            scale,
            alpha,
            Kind::Unspecified,
        )
    }

    fn render_popup_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<WaylandSurfaceRenderElement<R>> {
        let Some(surface) = self.wl_surface() else {
            return vec![];
        };
        PopupManager::popups_for_surface(&surface)
            .flat_map(|(popup, popup_offset)| {
                let offset = (popup_offset - popup.geometry().loc).to_physical_precise_round(scale);

                render_elements_from_surface_tree(
                    renderer,
                    popup.wl_surface(),
                    location + offset,
                    scale,
                    alpha,
                    Kind::Unspecified,
                )
            })
            .collect()
    }

    fn set_offscreen_element_id(&self, id: Option<smithay::backend::renderer::element::Id>) {
        self.user_data()
            .insert_if_missing(|| WindowOffscreenId(RefCell::new(None)));
        let mut guard = self
            .user_data()
            .get::<WindowOffscreenId>()
            .unwrap()
            .0
            .borrow_mut();
        *guard = id;
    }

    fn get_offscreen_element_id(&self) -> Option<smithay::backend::renderer::element::Id> {
        let guard = self.user_data().get::<WindowOffscreenId>()?.0.borrow_mut();
        guard.clone()
    }
}
