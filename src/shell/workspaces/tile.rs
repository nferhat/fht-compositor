use std::time::Duration;

use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::desktop::space::SpaceElement;
use smithay::desktop::{PopupManager, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Monotonic, Physical, Point, Rectangle, Scale, Size, Time};
use smithay::wayland::compositor::{with_surface_tree_downward, TraversalAction};
use smithay::wayland::seat::WaylandFocus;

use crate::config::{BorderConfig, CONFIG};
use crate::renderer::extra_damage::ExtraDamage;
use crate::renderer::pixel_shader_element::FhtPixelShaderElement;
use crate::renderer::rounded_element::RoundedCornerElement;
use crate::renderer::rounded_outline_shader::{RoundedOutlineElement, RoundedOutlineSettings};
use crate::renderer::texture_element::FhtTextureElement;
use crate::renderer::{FhtRenderer, SplitRenderElements};
use crate::utils::animation::Animation;
use crate::utils::geometry::{
    Local, PointGlobalExt, PointLocalExt, RectExt, SizeExt,
};

#[allow(unused)] // I did not finish implementing everything using this trait.
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
    ) -> SplitRenderElements<WaylandSurfaceRenderElement<R>>;
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

    /// Since we clip our tile damage for rounded corners, we still have to damage these regions.
    /// This is achieved using this.
    pub rounded_corner_damage: ExtraDamage,

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
        Self {
            element,
            location: Point::default(),
            cfact: 1.0,
            border_config,
            rounded_corner_damage: ExtraDamage::default(),
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
    pub fn set_geometry(&mut self, mut new_geo: Rectangle<i32, Local>, animate: bool) {
        if self.need_border() {
            let thickness = self.border_config().thickness as i32;
            new_geo.loc += (thickness, thickness).into();
            new_geo.size -= (2 * thickness, 2 * thickness).into();
        }

        self.element.set_size(new_geo.size);
        self.element.send_pending_configure();
        self.rounded_corner_damage
            .set_size(new_geo.size.as_logical());

        // Location animation
        //
        // We set our actual location, then we offset gradually until we reach our destination.
        // By that point our offset should be equal to 0
        let old_location = self.location;
        self.location = new_geo.loc;
        if animate {
            self.location_animation = Animation::new(
                old_location - new_geo.loc,
                Point::default(),
                CONFIG.animation.window_geometry.curve,
                Duration::from_millis(CONFIG.animation.window_geometry.duration),
            );
        }
    }

    /// Send a pending configure message to the window
    pub fn send_pending_configure(&mut self) {
        self.element.send_pending_configure();
    }

    /// Get this tile's geometry.
    pub fn geometry(&self) -> Rectangle<i32, Local> {
        let mut geo = self.element.geometry().as_local();
        geo.loc = self.location;
        geo
    }

    /// Get this tile's visual geometry, the geometry containing actual window content, excluding
    /// for example shadows.
    pub fn visual_geometry(&self) -> Rectangle<i32, Local> {
        let mut geo = self.element.geometry().as_local();
        geo.loc = self.render_location();
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
        let mut render_location = self.temporary_render_location.unwrap_or(self.location);
        render_location -= self.element.render_location_offset();

        if let Some(offset) = self.location_animation.as_ref().map(Animation::value) {
            render_location += offset;
        }

        render_location
    }

    /// Return whether we need to draw a border for this tile.
    pub fn need_border(&self) -> bool {
        !self.element.fullscreen()
    }

    /// Return whether we need to round this tile.
    pub fn need_rounding(&self) -> bool {
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

    /// Return whether this tile contains this [`WlSurface`] of [`WindowSurfaceType`]
    pub fn has_surface(&self, surface: &WlSurface, surface_type: WindowSurfaceType) -> bool {
        let element_surface = self.element.wl_surface().unwrap();
        if surface_type.contains(WindowSurfaceType::TOPLEVEL) && &*element_surface == surface {
            return true;
        }

        if surface_type.contains(WindowSurfaceType::SUBSURFACE) {
            use std::sync::atomic::{AtomicBool, Ordering}; // thank you.

            let found_surface: AtomicBool = false.into();
            with_surface_tree_downward(
                &element_surface,
                surface,
                |_, _, e| TraversalAction::DoChildren(e),
                |s, _, search| {
                    found_surface.fetch_or(s == *search, Ordering::SeqCst);
                },
                |_, _, _| !found_surface.load(Ordering::SeqCst),
            );
            if found_surface.load(Ordering::SeqCst) {
                return true;
            }
        }

        if surface_type.contains(WindowSurfaceType::POPUP) {
            return PopupManager::popups_for_surface(&element_surface)
                .any(|(popup, _)| popup.wl_surface() == surface);
        }

        false
    }

    /// Generate render elements for this tile.
    pub fn render_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        output: &Output,
        scale: Scale<f64>,
        alpha: f32,
        focused: bool,
    ) -> impl Iterator<Item = WorkspaceTileRenderElement<R>> {
        let render_location = self.render_location().to_global(output).as_logical();
        let render_location_phys = self
            .render_location()
            .to_global(&output)
            .as_logical()
            .to_physical_precise_round(scale);
        // Our tile visual geometry, this will be used to crop out rounded corners
        let tile_geo = Rectangle::from_loc_and_size(
            render_location + self.element.render_location_offset().as_logical(),
            self.element.size().as_logical(),
        );

        let border_config = self.border_config();
        let need_border = self.need_border();
        let need_rounding = self.need_rounding();

        let window_elements =
            self.element
                .render_elements(renderer, render_location_phys, scale, alpha);

        let mut need_extra_damage = false;
        let surface_elements = window_elements
            .normal
            .into_iter()
            .map(|e| {
                if !need_rounding {
                    return WorkspaceTileRenderElement::Element(e);
                }

                // Rounding off windows is a little tricky.
                //
                // Not every surface of the window means its "the window", not at all.
                // Some clients (like OBS-studio) use subsurfaces (not popups) to display different
                // parts of their interface (for example OBs does this with the preview window)
                //
                // To counter this, we check here if the surface is going to clip.
                if RoundedCornerElement::will_clip(&e, scale, tile_geo, border_config.radius) {
                    let rounded =
                        RoundedCornerElement::new(e, border_config.radius(), tile_geo, scale);
                    need_extra_damage = true;
                    WorkspaceTileRenderElement::RoundedElement(rounded)
                } else {
                    WorkspaceTileRenderElement::Element(e)
                }
            })
            .collect::<Vec<_>>();

        // If we are rendering a border, it will act as our damage, otherwise, with no border, we
        // need to damage ourselves
        let damage = (need_extra_damage && border_config.thickness == 0)
            .then(|| {
                let damage = self.rounded_corner_damage.clone().with_location(
                    (self.render_location() + self.element.render_location_offset()).as_logical(),
                );
                WorkspaceTileRenderElement::RoundedElementDamage(damage)
            })
            .into_iter();

        let border_element = need_border
            .then(|| {
                let border_location =
                    render_location + self.element.render_location_offset().as_logical();
                let mut border_geo =
                    Rectangle::from_loc_and_size(border_location, self.element.size().as_logical());
                let thickness = border_config.thickness as i32;
                border_geo.loc -= (thickness, thickness).into();
                border_geo.size += (2 * thickness, 2 * thickness).into();

                let border_element = RoundedOutlineElement::element(
                    renderer,
                    scale.x.max(scale.y),
                    alpha,
                    border_geo,
                    RoundedOutlineSettings {
                        half_thickness: border_config.half_thickness(),
                        radius: border_config.radius(),
                        color: if focused {
                            border_config.focused_color
                        } else {
                            border_config.normal_color
                        },
                    },
                );

                WorkspaceTileRenderElement::Border(border_element)
            })
            .into_iter();

        window_elements
            .popups
            .into_iter()
            .map(WorkspaceTileRenderElement::Element)
            .chain(damage)
            .chain(border_element)
            .chain(surface_elements)
    }
}

crate::fht_render_elements! {
    WorkspaceTileRenderElement<R> => {
        Element = WaylandSurfaceRenderElement<R>,
        RoundedElement = RoundedCornerElement<WaylandSurfaceRenderElement<R>>,
        RoundedElementDamage = ExtraDamage,
        Border = FhtPixelShaderElement,
        // Rescaling magic is done pretty weirdly:
        //
        // We render everything above then put everything inside a texture element.
        // Then, we actually rescale the texture.
        Rescaling = RescaleRenderElement<FhtTextureElement>,
    }
}
