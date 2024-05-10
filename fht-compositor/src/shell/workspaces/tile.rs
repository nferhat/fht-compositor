use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::desktop::space::SpaceElement;
use smithay::reexports::wayland_server::protocol::wl_output;
use smithay::utils::{IsAlive, Physical, Point, Rectangle, Scale, Size};
use smithay::wayland::seat::WaylandFocus;

use crate::config::{BorderConfig, CONFIG};
use crate::renderer::custom_texture_shader_element::CustomTextureShaderElement;
use crate::renderer::pixel_shader_element::FhtPixelShaderElement;
use crate::renderer::rounded_outline_shader::{RoundedOutlineShader, RoundedOutlineShaderSettings};
use crate::renderer::FhtRenderer;
use crate::utils::geometry::{Local, PointLocalExt, RectExt};

pub trait WorkspaceElement:
    Clone + std::fmt::Debug + SpaceElement + WaylandFocus + IsAlive + Sized + PartialEq
{
    /// Get the unique ID of this element, to identify it in the D-Bus IPC>
    fn uid(&self) -> u64;

    /// Send a configure message to this element.
    ///
    /// Wayland works by accumulating changes between commits and then when either the XDG toplevel
    /// window or the server/compositor send a configure message, the changes are then applied.
    fn send_pending_configure(&self);

    /// Get the render location offset of this element.
    ///
    /// Some clients like to draw client-side decorations such as titlebars, shadows, etc. If they
    /// do so, the location of our client should be offsetted to account for these CSDs.
    ///
    /// This function returns the necessary offset to account for them.
    fn render_location_offset(&self) -> Point<i32, Local>;

    /// Set the size of this element.
    fn set_size(&self, new_size: Size<i32, Local>);
    /// Get the size of this element.
    fn size(&self) -> Size<i32, Local>;

    /// Set whether this element is fullscreened or not.
    fn set_fullscreen(&self, fullscreen: bool);
    /// Set the fullscreen output for this element.
    fn set_fullscreen_output(&self, output: Option<wl_output::WlOutput>);
    /// Get whether the this element is fullscreened or not.
    fn fullscreen(&self) -> bool;
    /// Get the fullscreen output of this element.
    fn fullscreen_output(&self) -> Option<wl_output::WlOutput>;

    /// Set whether this element is maximized or not.
    fn set_maximized(&self, maximize: bool);
    /// Get whether the this element is maximizeed or not.
    fn maximized(&self) -> bool;

    /// Set the bounds of this element.
    fn set_bounds(&self, bounds: Option<Size<i32, Local>>);
    /// Get the bounds of this element.
    fn bounds(&self) -> Option<Size<i32, Local>>;

    /// Set whether this element is activated or not.
    fn set_activated(&self, activated: bool);
    /// Get whether this element is activated or not.
    fn activated(&self) -> bool;

    /// Get the app_id/class of this element.
    fn app_id(&self) -> String;
    /// Get the title of this element.
    fn title(&self) -> String;

    /// Generate render elements for this element at a given location.
    ///
    /// The render elements should account for CSD: in other terms `location` should match the
    /// usable position of the client.
    fn render_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<WorkspaceTileRenderElement<R>>;
}

/// A single workspace tile.
///
/// A workspace tile is responsible for managing an inner [`WorkspaceElement`] by giving a
/// position, border, and other properties. This tile is useful only if you store it inside a
/// [`Workspace`](super::Workspace)
#[derive(Debug)]
pub struct WorkspaceTile<E: WorkspaceElement> {
    /// The inner element.
    pub(super) element: E,

    /// The location of this element, relative to the workspace holding it.
    ///
    /// This location should be the top left corner of the tile's element, in other terms excluding
    /// the client-side decorations
    pub location: Point<i32, Local>,

    /// The currently client fact added to this tile.
    ///
    /// This float being higher means that this tile of the workspace will take more or less
    /// relative space (width/height, based on the layout) of its stack based on its neighbours
    /// cfacts.
    pub cfact: f32,

    /// The Z-index of this element,
    pub z_index: usize,

    /// The border configuration for this tile.
    ///
    /// This can be user specified using window rules, falling back to the global configuration if
    /// not set.
    pub border_config: Option<BorderConfig>,
    // TODO: Move animations to this struct.
}

impl<E: WorkspaceElement> PartialEq for WorkspaceTile<E> {
    fn eq(&self, other: &Self) -> bool {
        self.element == other.element
    }
}

impl<E: WorkspaceElement> PartialEq<E> for WorkspaceTile<E> {
    fn eq(&self, other: &E) -> bool {
        self.element == *other
    }
}

impl<E: WorkspaceElement> WorkspaceTile<E> {
    /// Get a reference to this tile's inner element.
    pub fn element(&self) -> &E {
        &self.element
    }

    /// Get this tile's geometry.
    pub fn geometry(&self) -> Rectangle<i32, Local> {
        let mut geo = self.element.geometry().as_local();
        geo.loc = self.location;
        geo
    }

    /// Get this tile's bounding box.
    pub fn bbox(&self) -> Rectangle<i32, Local> {
        let mut bbox = self.element.bbox().as_local();
        bbox.loc = self.location;
        bbox
    }

    /// Get this tile's render location.
    pub fn render_location(&self) -> Point<i32, Local> {
        self.location - self.element.render_location_offset()
    }

    /// Return whether we need to draw a border for this tile.
    pub fn need_border(&self) -> bool {
        !self.element.fullscreen()
    }

    /// Return the border settings to use when rendering this tile.
    pub fn border_config(&self) -> BorderConfig {
        self.border_config.unwrap_or(CONFIG.decoration.border)
    }

    /// Generate render elements for this tile.
    pub fn render_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        scale: Scale<f64>,
        alpha: f32,
        focused: bool,
    ) -> Vec<WorkspaceTileRenderElement<R>> {
        let render_location = self
            .render_location()
            .as_logical()
            .to_physical_precise_round(scale);
        let mut render_elements =
            self.element
                .render_elements(renderer, render_location, scale, alpha);

        if self.need_border() {
            let mut border_geo = Rectangle::from_loc_and_size(self.location, self.element.size());
            let border_config = self.border_config();
            let thickness = border_config.thickness as i32;
            border_geo.loc -= (thickness, thickness).into();
            border_geo.size += (2 * thickness, 2 * thickness).into();

            let border_element = RoundedOutlineShader::element(
                renderer,
                scale.x.max(scale.y),
                alpha,
                self.element.wl_surface().as_ref().unwrap(),
                border_geo,
                RoundedOutlineShaderSettings {
                    thickness: thickness as u8,
                    radius: border_config.radius,
                    color: if focused {
                        border_config.focused_color
                    } else {
                        border_config.normal_color
                    },
                },
            );

            render_elements.push(WorkspaceTileRenderElement::Border(border_element))
        }

        render_elements
    }
}

crate::fht_render_elements! {
    WorkspaceTileRenderElement<R> => {
        Element = CustomTextureShaderElement<WaylandSurfaceRenderElement<R>>,
        Border = FhtPixelShaderElement,
    }
}
