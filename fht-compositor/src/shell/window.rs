use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::desktop::{PopupManager, Window};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Physical, Point, Scale, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use super::workspaces::tile::{WorkspaceElement, WorkspaceTileRenderElement};
use crate::renderer::custom_texture_shader_element::CustomTextureShaderElement;
use crate::renderer::FhtRenderer;
use crate::utils::geometry::{Local, PointExt, PointLocalExt, SizeExt};

impl WorkspaceElement for Window {
    fn uid(&self) -> u64 {
        self.toplevel().unwrap().wl_surface().id().protocol_id() as u64
    }

    fn send_pending_configure(&self) {
        self.toplevel().unwrap().send_pending_configure();
    }

    fn render_location_offset(&self) -> Point<i32, Local> {
        self.geometry().loc.as_local()
    }

    fn set_size(&self, new_size: smithay::utils::Size<i32, Local>) {
        self.toplevel().unwrap().with_pending_state(|state| {
            state.size = Some(new_size.as_logical());
        });
    }

    fn size(&self) -> Size<i32, Local> {
        self.geometry().size.as_local()
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

    fn set_bounds(&self, bounds: Option<Size<i32, Local>>) {
        self.toplevel().unwrap().with_pending_state(|state| {
            state.bounds = bounds.map(Size::as_logical);
        });
    }

    fn bounds(&self) -> Option<Size<i32, Local>> {
        self.toplevel()
            .unwrap()
            .with_pending_state(|state| state.bounds.map(Size::as_local))
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

    fn render_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<WorkspaceTileRenderElement<R>> {
        let surface = self.wl_surface().unwrap();
        let render_offset = self.render_location_offset().as_logical();

        let mut render_elements = PopupManager::popups_for_surface(&surface)
            .flat_map(|(popup, popup_offset)| {
                let offset = (render_offset + popup_offset - popup.geometry().loc)
                    .to_physical_precise_round(scale);

                render_elements_from_surface_tree::<_, WaylandSurfaceRenderElement<R>>(
                    renderer,
                    popup.wl_surface(),
                    location + offset,
                    scale,
                    alpha,
                    Kind::Unspecified,
                )
                .into_iter()
                .map(CustomTextureShaderElement::from_element_no_shader)
                .map(WorkspaceTileRenderElement::Element)
            })
            .collect::<Vec<_>>();

        render_elements.extend(
            render_elements_from_surface_tree(
                renderer,
                &surface,
                location,
                scale,
                alpha,
                Kind::Unspecified,
            )
            .into_iter()
            .map(CustomTextureShaderElement::from_element_no_shader)
            .map(WorkspaceTileRenderElement::Element),
        );

        render_elements
    }
}
