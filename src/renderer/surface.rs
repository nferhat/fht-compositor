use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
use smithay::backend::renderer::{ImportAll, Renderer};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size};
use smithay::wayland::compositor::{TraversalAction, with_states, with_surface_tree_downward};

/// Generate the [`WaylandSurfaceRenderElement`]s from this [`WlSurface`] tree, and push them using
/// the passed in `push` function. Adapted from Smithay's `render_elements_from_surface_tree`.
pub fn push_elements_from_surface_tree<R>(
    renderer: &mut R,
    surface: &WlSurface,
    // Fractional scale expects surface buffers to be aligned to physical pixels.
    location: Point<i32, Physical>,
    scale: impl Into<Scale<f64>>,
    alpha: f32,
    kind: Kind,
    push: &mut dyn FnMut(WaylandSurfaceRenderElement<R>),
) where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    crate::profile_function!();
    let location = location.to_f64();
    let scale = scale.into();

    with_surface_tree_downward(
        surface,
        location,
        |_, states, location| {
            let mut location = *location;
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            if let Some(data) = data {
                if let Some(view) = data.lock().unwrap().view() {
                    location += view.offset.to_f64().to_physical(scale);
                    TraversalAction::DoChildren(location)
                } else {
                    TraversalAction::SkipChildren
                }
            } else {
                TraversalAction::SkipChildren
            }
        },
        |surface, states, location| {
            let mut location = *location;
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            if let Some(data) = data {
                let has_view = if let Some(view) = data.lock().unwrap().view() {
                    location += view.offset.to_f64().to_physical(scale);
                    true
                } else {
                    false
                };

                if has_view {
                    match WaylandSurfaceRenderElement::from_surface(
                        renderer, surface, states, location, alpha, kind,
                    ) {
                        Ok(Some(surface)) => push(surface),
                        Ok(None) => {} // surface is not mapped
                        Err(err) => {
                            warn!("failed to import surface: {}", err);
                        }
                    };
                }
            }
        },
        |_, _, _| true,
    );
}

#[allow(unused)]
/// Get whether a surface has any transparent region. This is calculated from opaque regions
/// provided by the surface aswell as the render format.
pub fn has_transparent_region(surface: &WlSurface, surface_size: Size<i32, Logical>) -> bool {
    // Opaque regions are described in surface-local coordinates.
    let surface_geo = Rectangle::from_size(surface_size);
    with_states(surface, |data| {
        let Some(renderer_data) = data.data_map.get::<RendererSurfaceStateUserData>() else {
            return false; // cannot check here yet.
        };
        let renderer_data = renderer_data.lock().unwrap();
        if let Some(opaque_regions) = renderer_data.opaque_regions() {
            // If there's some place left after removing opaque regions, these are
            // transparent regions and must be rendered using blur.
            let remaining = surface_geo.subtract_rects(opaque_regions.iter().copied());
            !remaining.is_empty()
        } else {
            // no opaque regions == fully transparent window surface
            true
        }
    })
}
