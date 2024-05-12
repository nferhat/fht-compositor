use std::time::Duration;

use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::desktop::space::SpaceElement;
use smithay::reexports::wayland_server::protocol::wl_output;
use smithay::utils::{IsAlive, Monotonic, Physical, Point, Rectangle, Scale, Size, Time};
use smithay::wayland::seat::WaylandFocus;

use crate::config::{BorderConfig, ColorConfig, CONFIG};
use crate::renderer::custom_texture_shader_element::CustomTextureShaderElement;
use crate::renderer::pixel_shader_element::FhtPixelShaderElement;
use crate::renderer::rounded_outline_shader::{RoundedOutlineShader, RoundedOutlineShaderSettings};
use crate::renderer::texture_element::FhtTextureElement;
use crate::renderer::FhtRenderer;
use crate::utils::animation::Animation;
use crate::utils::geometry::{Local, PointLocalExt, RectExt, SizeExt};

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
    ///
    /// The element should not send a configure message with this.
    fn set_size(&self, new_size: Size<i32, Local>);
    /// Get the size of this element.
    fn size(&self) -> Size<i32, Local>;

    /// Set whether this element is fullscreened or not.
    ///
    /// The element should not send a configure message with this.
    fn set_fullscreen(&self, fullscreen: bool);
    /// Set the fullscreen output for this element.
    ///
    /// The element should not send a configure message with this.
    fn set_fullscreen_output(&self, output: Option<wl_output::WlOutput>);
    /// Get whether the this element is fullscreened or not.
    fn fullscreen(&self) -> bool;
    /// Get the fullscreen output of this element.
    fn fullscreen_output(&self) -> Option<wl_output::WlOutput>;

    /// Set whether this element is maximized or not.
    ///
    /// The element should not send a configure message with this.
    fn set_maximized(&self, maximize: bool);
    /// Get whether the this element is maximizeed or not.
    fn maximized(&self) -> bool;

    /// Set the bounds of this element.
    ///
    /// The element should not send a configure message with this.
    fn set_bounds(&self, bounds: Option<Size<i32, Local>>);
    /// Get the bounds of this element.
    fn bounds(&self) -> Option<Size<i32, Local>>;

    /// Set whether this element is activated or not.
    ///
    /// The element should not send a configure message with this.
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
    pub(crate) element: E,

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

    /// The border configuration for this tile.
    ///
    /// This can be user specified using window rules, falling back to the global configuration if
    /// not set.
    pub border_config: Option<BorderConfig>,

    /// The solid color buffer used when dragging this tile away from its location
    background_buffer: SolidColorBuffer,
    background_buffer_color: [f32; 4],

    /// The temporary render location of this tile.
    /// Used when dragging it using MoveTile mouse action.
    pub temporary_render_location: Option<Point<i32, Local>>,

    /// Location animation
    ///
    /// This value should be an offset getting closer to zero.
    pub location_animation: Option<Animation<Point<i32, Local>>>,
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
    /// Create a new tile.
    pub fn new(element: E, border_config: Option<BorderConfig>) -> Self {
        let buffer_size = element.geometry().size;
        let mut buffer_color = border_config
            .as_ref()
            .map(|cfg| cfg.focused_color.components())
            .unwrap_or(CONFIG.decoration.border.focused_color.components());
        buffer_color[0] *= 0.5; // arbitrary value, just making the inner border dimmer.
        buffer_color[1] *= 0.5; // yeah pretty lame, I know right.
        buffer_color[2] *= 0.5; //                             - nferhat
        buffer_color[3] *= 0.5; // TODO: Instead of using a Solid color buffer use a shader instead
                                // (users can write patterns idk...)

        let background_buffer = SolidColorBuffer::new(buffer_size, buffer_color);

        Self {
            element,
            location: Point::default(),
            cfact: 1.0,
            border_config: None,
            background_buffer,
            background_buffer_color: buffer_color,
            temporary_render_location: None,
            location_animation: None,
        }
    }

    /// Get a reference to this tile's inner element.
    pub fn element(&self) -> &E {
        &self.element
    }

    /// Set this tile's geometry.
    ///
    /// The tile automatically accounts for border geometry if it needs to.
    pub fn set_geometry(&mut self, mut new_geo: Rectangle<i32, Local>) {
        if self.need_border() {
            let thickness = self.border_config().thickness as i32;
            new_geo.loc += (thickness, thickness).into();
            new_geo.size -= (2 * thickness, 2 * thickness).into();
        }

        self.element.set_size(new_geo.size);
        self.element.send_pending_configure();
        self.background_buffer.resize(new_geo.size.as_logical());

        // Location animation
        //
        // We set our actual location, then we offset gradually until we reach our destination.
        // By that point our offset should be equal to 0
        let old_location = self.location;
        self.location = new_geo.loc;
        self.location_animation = Animation::new(
            old_location - new_geo.loc,
            Point::default(),
            CONFIG.animation.window_geometry.curve,
            Duration::from_millis(CONFIG.animation.window_geometry.duration),
        );

    }

    /// Set this tile's geometry without animating.
    ///
    /// See [`Self::set_geometry`]
    pub fn set_geometry_instant(&mut self, mut new_geo: Rectangle<i32, Local>) {
        if self.need_border() {
            let thickness = self.border_config().thickness as i32;
            new_geo.loc += (thickness, thickness).into();
            new_geo.size -= (2 * thickness, 2 * thickness).into();
        }

        self.element.set_size(new_geo.size);
        self.element.send_pending_configure();
        self.background_buffer.resize(new_geo.size.as_logical());
        self.location = new_geo.loc;
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
        let mut render_location = self
            .temporary_render_location
            .unwrap_or(self.location);
        render_location -= self.element.render_location_offset();

        if let Some(offset) = self.location_animation.as_ref().map(Animation::value) {
            render_location += offset;
        }

        render_location
    }

    /// Return whether we need to draw the placeholder background buffer.
    pub fn need_background_buffer(&self) -> bool {
        self.temporary_render_location.is_some()
    }


    /// Return whether the workspace holding this tile should draw it above others.
    pub fn draw_above_others(&self) -> bool {
        self.temporary_render_location.is_some() || self.element.activated()
    }

    /// Return whether we need to draw a border for this tile.
    pub fn need_border(&self) -> bool {
        !self.element.fullscreen()
    }

    /// Return the border settings to use when rendering this tile.
    pub fn border_config(&self) -> BorderConfig {
        self.border_config.unwrap_or(CONFIG.decoration.border)
    }

    /// Advance this tile's animations.
    pub fn advance_animations(&mut self, current_time: Time<Monotonic>) -> bool {
        let _ = self.location_animation.take_if(|anim| anim.is_finished());
        if let Some(location_animation) = self.location_animation.as_mut() {
            location_animation.set_current_time(current_time);
            return true;
        }

        false
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
            let border_location = self.render_location() + self.element.render_location_offset();
            let mut border_geo = Rectangle::from_loc_and_size(border_location, self.element.size());
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

        if self.need_background_buffer() {
            let render_location = self.location.as_logical().to_physical_precise_round(scale); // render it where the border should be.
            let render_element = SolidColorRenderElement::from_buffer(
                &self.background_buffer,
                render_location,
                scale,
                alpha,
                Kind::Unspecified,
            );
            render_elements.push(WorkspaceTileRenderElement::Background(render_element));

            // Render again where the buffer is
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
                    color: ColorConfig::Solid([
                        self.background_buffer_color[0] * 1.5,
                        self.background_buffer_color[1] * 1.5,
                        self.background_buffer_color[2] * 1.5,
                        self.background_buffer_color[3] * 1.5,
                    ]),
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
        Background = SolidColorRenderElement,
        Border = FhtPixelShaderElement,
        // Rescaling magic is done pretty weirdly:
        //
        // We render everything above then put everything inside a texture element.
        // Then, we actually rescale the texture.
        Rescaling = RescaleRenderElement<FhtTextureElement>,
    }
}
