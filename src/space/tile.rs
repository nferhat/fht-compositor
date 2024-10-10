use std::rc::Rc;
use std::time::Duration;

use fht_animation::Animation;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::{Element as _, Id, Kind};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::Renderer as _;
use smithay::desktop::{PopupManager, WindowSurfaceType};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::compositor::{with_surface_tree_downward, TraversalAction};
use smithay::wayland::seat::WaylandFocus;

use super::closing_tile::ClosingTile;
use super::Config;
use crate::egui::EguiRenderElement;
use crate::renderer::extra_damage::ExtraDamage;
use crate::renderer::pixel_shader_element::FhtPixelShaderElement;
use crate::renderer::rounded_element::RoundedCornerElement;
use crate::renderer::texture_element::FhtTextureElement;
use crate::renderer::{render_to_texture, FhtRenderer};
use crate::utils::RectCenterExt;
use crate::window::Window;

/// A single workspace tile.
///
/// By itself, a [`Window`] does not hold any data about how its being managed inside the compositor
/// space. A [`Tile`] associates a given window with additional that allows it to be mapped and
/// managed by a [`Workspace`]
#[derive(Debug)]
pub struct Tile {
    /// The [`Window`] that this tile manages.
    window: Window,

    /// The location of this tile inside the [`Workspace`] that holds him.
    ///
    /// This is the location of the tile's top-left corner, so this location accounts for the
    /// tile's border. For the window location relative to a [`Workspace`], add
    /// [`Tile::window_loc`].
    location: Point<i32, Logical>,

    /// The proportion of this [`Tile`] relative to others in its stack.
    proportion: f64,

    /// The current location animation of this tile.
    ///
    /// This animation value's (if any) gets added to `self.location` in order to get the final
    /// location where to render the tile.
    location_animation: Option<LocationAnimation>,

    /// Extra damage bag to apply when the tile corners are being rounded.
    /// This is due to an implementation detail of [`RoundedCornerElement`]
    extra_damage: ExtraDamage,

    /// The current opening animation.
    ///
    /// This affects the [`Tile`]'s final scale and opacity, in order to give a pop-in effect.
    opening_animation: Option<Animation<f64>>,

    /// Prepared render elements for a [`ClosingTile`].
    ///
    /// These are rendered the frame before `self.window` removes/unmaps its buffers, in order to
    /// create a "snapshot" of the window to animate the [`ClosingTile`]
    close_animation_snapshot: Option<Vec<TileRenderElement<GlowRenderer>>>,

    /// Configuration specific to the workspace system.
    pub config: Rc<Config>,
}

crate::fht_render_elements! {
    TileRenderElement<R> => {
        Surface = WaylandSurfaceRenderElement<R>,
        RoundedSurface = RoundedCornerElement<WaylandSurfaceRenderElement<R>>,
        RoundedSurfaceDamage = ExtraDamage,
        Border = FhtPixelShaderElement,
        DebugOverlay = EguiRenderElement,
        Opening = RescaleRenderElement<FhtTextureElement>,
    }
}

impl Tile {
    /// Create a new [`Tile`] from this window.
    pub fn new(window: Window, config: Rc<Config>) -> Self {
        let proportion = window.rules().proportion.unwrap_or(1.0);

        Self {
            window,
            location: Point::default(),
            proportion,
            location_animation: None,
            opening_animation: None,
            extra_damage: ExtraDamage::default(),
            close_animation_snapshot: None,
            config,
        }
    }

    /// Get a reference to the [`Window`] of this [`Tile`].
    pub const fn window(&self) -> &Window {
        &self.window
    }

    /// Deconstructs this [`Tile`] into a [`Window`].
    pub fn into_window(self) -> Window {
        self.window
    }

    /// Deconstructs this [`Tile`] into a [`ClosingTile`], if the window open close animation is
    /// enabled.
    pub fn into_closing_tile(
        mut self,
        renderer: &mut GlowRenderer,
        scale: Scale<f64>,
    ) -> Option<ClosingTile> {
        let render_elements = self.close_animation_snapshot.take()?;
        let animation = self.config.window_open_close_animation.as_ref()?;
        let geometry = self.visual_geometry();
        Some(ClosingTile::new(
            renderer,
            render_elements,
            geometry,
            scale,
            animation,
        ))
    }

    /// Get the proportion of this [`Tile`].
    pub const fn proportion(&self) -> f64 {
        self.proportion
    }

    /// Set the proportion of this [`Tile`].
    pub fn set_proportion(&mut self, proportion: f64) {
        self.proportion = proportion;
    }

    /// Check if this [`Tile`] contains this [`WlSurface`] in its surface tree.
    pub fn has_surface(&self, s: &WlSurface, surface_type: WindowSurfaceType) -> bool {
        let Some(window_surface) = self.window.wl_surface() else {
            return false;
        };
        let window_surface = window_surface.as_ref();

        if surface_type.contains(WindowSurfaceType::TOPLEVEL) && window_surface == s {
            return true;
        }

        if surface_type.contains(WindowSurfaceType::SUBSURFACE) {
            use std::cell::RefCell;
            let found = RefCell::new(false);
            with_surface_tree_downward(
                &window_surface,
                s,
                |_, _, e| TraversalAction::DoChildren(e),
                |s, _, search| {
                    *found.borrow_mut() |= s == *search;
                },
                |_, _, _| *found.borrow(),
            );

            if *found.borrow() {
                return true;
            }
        }

        if surface_type.contains(WindowSurfaceType::POPUP) {
            return PopupManager::popups_for_surface(&window_surface)
                .any(|(popup, _)| popup.wl_surface() == s);
        }

        false
    }

    /// Set this [`Tile`]'s geometry.
    ///
    /// The `new_geometry` argument will the geometry of the whole [`Tile`], including its border.
    ///
    /// This does not call [`Window::send_configure`]!
    pub fn set_geometry(&mut self, new_geometry: Rectangle<i32, Logical>, animate: bool) {
        self.set_location(new_geometry.loc, animate);
        self.set_size(new_geometry.size, animate);
    }

    /// Get this [`Tile`]'s geometry, in other words its effective [`Rectangle`] in compositor space.
    ///
    /// The returned [`Rectangle`] is the geometry of the whole [`Tile`], including its border.
    ///
    /// This accounts for any ongoing location animation.
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        let window_size = self.window.size();
        let border_thickness = if self.window.fullscreen() {
            0 // No border is drawn when the window is fullscreened.
        } else {
            let rules = self.window.rules();
            // TODO: Use the actual border thickness when we reach fractional layout.
            self.config
                .border
                .with_overrides(&rules.border_overrides)
                .thickness as i32
        };
        let tile_size = Size::from((
            window_size.w + 2 * border_thickness,
            window_size.h + 2 * border_thickness,
        ));

        Rectangle::from_loc_and_size(self.location, tile_size)
    }

    /// Get this [`Tile`]'s visual geometry, in other words where the [`Tile`]'s [`Rectangle`] will
    /// end up at when rendering.
    ///
    /// The returned [`Rectangle`] is the geometry of the whole [`Tile`], including its border.
    ///
    /// This accounts for any ongoing location animation.
    pub fn visual_geometry(&self) -> Rectangle<i32, Logical> {
        let window_size = self.window.size();
        let border_thickness = if self.window.fullscreen() {
            0 // No border is drawn when the window is fullscreened.
        } else {
            let rules = self.window.rules();
            // TODO: Use the actual border thickness when we reach fractional layout.
            self.config
                .border
                .with_overrides(&rules.border_overrides)
                .thickness as i32
        };
        let mut tile_location = self.location;
        if let Some(animation) = &self.location_animation {
            tile_location += animation.value();
        }
        let tile_size = Size::from((
            window_size.w + 2 * border_thickness,
            window_size.h + 2 * border_thickness,
        ));

        Rectangle::from_loc_and_size(tile_location, tile_size)
    }

    /// Set this [`Tile`]'s location.
    ///
    /// The `new_location` argument will the location of the whole [`Tile`], including its border.
    pub fn set_location(&mut self, new_location: Point<i32, Logical>, animate: bool) {
        if let Some(window_geometry_animation) = &self.config.window_geometry_animation {
            let mut old_location = self.location;
            if let Some(previous_animation) = self.location_animation.take() {
                old_location += previous_animation.value();
            }
            self.location = new_location;
            if animate {
                self.location_animation =
                    LocationAnimation::new(old_location, new_location, window_geometry_animation);
            }
        } else {
            if self.location != new_location {
                self.location = new_location
            }
        }
    }

    /// Get this [`Tile`]'s visual location, in orther words where the tile is going to **render**.
    ///
    /// The returned value will the location of the whole [`Tile`], including its border.
    pub fn visual_location(&self) -> Point<i32, Logical> {
        let mut tile_location = self.location;
        if let Some(animation) = &self.location_animation {
            tile_location += animation.value();
        }
        tile_location
    }

    /// Get this [`Tile`]'s location, in other words its effective location in compositor space.
    ///
    /// The returned value will the location of the whole [`Tile`], including its border.
    pub fn location(&self) -> Point<i32, Logical> {
        self.location
    }

    /// Get the [`Window`]'s location relative to this [`Tile`].
    ///
    /// A [`Tile`] can have a border around it, so the actual window will get rendered inside the
    /// border, and not at `self.location`.
    pub fn window_loc(&self) -> Point<i32, Logical> {
        let mut loc = Point::default();
        if self.window.fullscreen() {
            // When we are fullscreened, we do not render the border
            return loc;
        }

        let rules = self.window.rules();
        let border = self.config.border.with_overrides(&rules.border_overrides);
        // TODO: Use the actual fractional value
        let thickness = border.thickness.round() as i32;
        loc.x += thickness;
        loc.y += thickness;

        loc
    }

    /// Set this [`Tile`]'s size.
    ///
    /// The `new_size` argument will the size of the whole [`Tile`], including its border.
    ///
    /// This does not call [`Window::send_configure`]!
    pub fn set_size(&mut self, new_size: Size<i32, Logical>, _animate: bool) {
        // TODO: window resize animations
        self.extra_damage.set_size(new_size);
        let rules = self.window.rules();
        // TODO: Use the actual border thickness when we reach fractional layout.
        let mut border_thickness = self
            .config
            .border
            .with_overrides(&rules.border_overrides)
            .thickness as i32;
        if self.window.fullscreen() {
            // When we have a fullscreen window, no border is drawn
            border_thickness = 0;
        }
        let window_size = Size::from((
            new_size.w - 2 * border_thickness,
            new_size.h - 2 * border_thickness,
        ));
        self.window.request_size(window_size);
    }

    /// Advance animations for this [`Tile`].
    pub fn advance_animations(&mut self, now: Duration) -> bool {
        let mut animations_ongoing = false;

        let _ = self.location_animation.take_if(|a| a.is_finished());
        if let Some(animation) = &mut self.location_animation {
            animations_ongoing = true;
            animation.tick(now);
        }

        let _ = self.opening_animation.take_if(|a| a.is_finished());
        if let Some(animation) = &mut self.opening_animation {
            animations_ongoing = true;
            animation.tick(now);
        }

        animations_ongoing
    }

    /// Start the opening animation for this [`Tile`], if the window open/close animation is
    /// enabled.
    pub fn start_opening_animation(&mut self) {
        let Some(animation) = &self.config.window_open_close_animation else {
            return;
        };

        self.opening_animation =
            Some(Animation::new(0.0, 1.0, animation.duration).with_curve(animation.curve));
    }

    /// Take a snapshot for running a [`ClosingTile`].
    pub fn prepare_close_animation_if_needed(
        &mut self,
        renderer: &mut GlowRenderer,
        scale: Scale<f64>,
    ) {
        if self.close_animation_snapshot.is_some() {
            return;
        }

        let elements = self.render_inner(renderer, (0, 0).into(), scale, false);
        self.close_animation_snapshot = Some(elements);
    }

    /// Clear the snapshot for running a [`ClosingTile`].
    pub fn clear_close_animation_snapshot(&mut self) {
        let _ = self.close_animation_snapshot.take();
    }

    /// Stop the ongoing location animation for this [`Tile`]
    pub fn stop_location_animation(&mut self) {
        let _ = self.location_animation.take();
    }

    /// Render the [`Tile`] elements, without giving.
    ///
    /// `location` is expected to be the top-left of the whole [`Tile`], including the border.
    fn render_inner<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        location: Point<i32, Logical>,
        scale: Scale<f64>,
        active: bool,
    ) -> Vec<TileRenderElement<R>> {
        let mut elements = vec![];
        let rules = self.window.rules();
        let is_fullscreen = self.window.fullscreen();

        let alpha = if is_fullscreen {
            1.0
        } else {
            rules.opacity.unwrap_or(1.0)
        };

        let border = self.config.border.with_overrides(&rules.border_overrides);
        let (border_thickness, border_radius) = if is_fullscreen {
            (0, 0.0)
        } else {
            (
                border.thickness.round() as i32,
                border.radius - (border.thickness / 2.0),
            )
        };

        drop(rules); // Avoid deadlock :skull:

        let window_geometry = Rectangle::from_loc_and_size(
            location + Point::<i32, Logical>::from((border_thickness, border_thickness)),
            self.window.size(),
        );
        let tile_geometry = Rectangle::from_loc_and_size(
            location,
            window_geometry.size + Size::from((border_thickness * 2, border_thickness * 2)),
        );

        if border_radius != 0.0 {
            let damage = self.extra_damage.clone().with_location(window_geometry.loc);
            elements.push(TileRenderElement::RoundedSurfaceDamage(damage));
        }

        let popup_elements = self
            .window
            .render_popup_elements(
                renderer,
                window_geometry.loc.to_physical_precise_round(scale),
                scale,
                alpha,
            )
            .into_iter()
            .map(TileRenderElement::Surface);
        elements.extend(popup_elements);

        let window_elements = self
            .window
            .render_toplevel_elements(
                renderer,
                window_geometry.loc.to_physical_precise_round(scale),
                scale,
                alpha,
            )
            .into_iter()
            .map(move |e| {
                // Rounding off windows is a little tricky.
                //
                // Not every surface of the window means its "the window", not at all.
                // Some clients (like OBS-studio) use subsurfaces (not popups) to display different
                // parts of their interface (for example OBs does this with the preview window)
                //
                // To counter this, we check here if the surface is going to clip.
                if RoundedCornerElement::will_clip(&e, scale.into(), window_geometry, border_radius)
                {
                    let rounded =
                        RoundedCornerElement::new(e, border_radius, window_geometry, scale.into());
                    TileRenderElement::RoundedSurface(rounded)
                } else {
                    TileRenderElement::Surface(e)
                }
            });
        elements.extend(window_elements);

        if border_thickness != 0 {
            let element = super::border::draw_border(
                renderer,
                // FIXME: Fractional scale woohoo
                scale.x.max(scale.y),
                alpha,
                tile_geometry,
                border_thickness as f64,
                border_radius as f64,
                if active {
                    border.focused_color
                } else {
                    border.normal_color
                },
            );
            elements.push(element.into());
        }

        elements
    }

    /// Render the elements for this [`Tile`].
    ///
    /// The [`Tile`] uses its `location` to render the elements.
    pub fn render<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        scale: f64,
        active: bool,
    ) -> impl Iterator<Item = TileRenderElement<R>> {
        let scale = Scale::from(scale);
        let mut opening_element = None;
        let mut normal_elements = vec![];

        if let Some(animation) = self.opening_animation.as_ref() {
            let render_geo = self.visual_geometry().to_physical_precise_round(scale);
            let progress = *animation.value();

            let glow_renderer = renderer.glow_renderer_mut();
            // NOTE: We use the border thickness as the location to actually include it with the
            // render elements, otherwise it would be clipped out of the tile.
            let elements = self.render_inner(glow_renderer, (0, 0).into(), scale, active);
            let rec = elements
                .iter()
                .fold(Rectangle::default(), |acc, e| acc.merge(e.geometry(scale)));

            opening_element = render_to_texture(
                glow_renderer,
                rec.size,
                scale,
                Transform::Normal,
                Fourcc::Abgr8888,
                elements.into_iter(),
            )
            .map_err(|err| {
                warn!(
                    ?err,
                    "Failed to render window elements to texture for open animation!"
                )
            })
            .ok()
            .map(|(texture, _sync_point)| {
                let glow_renderer = renderer.glow_renderer_mut();

                let element_id = Id::new();
                let texture: FhtTextureElement = TextureRenderElement::from_static_texture(
                    element_id.clone(),
                    glow_renderer.id(),
                    render_geo.loc.to_f64(),
                    texture,
                    // FIXME: This is garbage, "fractional scaling"
                    scale.x.max(scale.y).round() as i32,
                    Transform::Normal,
                    Some(progress.clamp(0., 1.) as f32),
                    None,
                    None,
                    None,
                    Kind::Unspecified,
                )
                .into();
                self.window.set_offscreen_element_id(Some(element_id));

                let origin = render_geo.center();
                let rescale = RescaleRenderElement::from_element(
                    texture,
                    origin,
                    opening_animation_progress_to_scale(progress),
                );

                TileRenderElement::<R>::Opening(rescale)
            });
        };

        if opening_element.is_none() {
            self.window.set_offscreen_element_id(None);
            normal_elements = self.render_inner(renderer, self.visual_location(), scale, active)
        }

        opening_element.into_iter().chain(normal_elements)
    }
}

fn opening_animation_progress_to_scale(progress: f64) -> f64 {
    const OPEN_SCALE_THRESHOLD: f64 = 0.5;
    progress * (1.0 - OPEN_SCALE_THRESHOLD) + OPEN_SCALE_THRESHOLD
}

#[derive(Debug)]
struct LocationAnimation {
    /// The x offset of this animation.
    x: Animation<i32>,
    /// The y offset of this animation.
    y: Animation<i32>,
}

impl LocationAnimation {
    /// Create a new [`LocationAnimation`]
    fn new(
        prev: Point<i32, Logical>,
        next: Point<i32, Logical>,
        config: &super::AnimationConfig,
    ) -> Option<Self> {
        if prev == next {
            return None;
        }

        let delta = prev - next;
        Some(Self {
            x: Animation::new(delta.x, 0, config.duration).with_curve(config.curve),
            y: Animation::new(delta.y, 0, config.duration).with_curve(config.curve),
        })
    }

    /// Tick this animation at a given [`Duration`].
    fn tick(&mut self, now: Duration) {
        self.x.tick(now);
        self.y.tick(now);
    }

    /// Check whether this animation finished or not.
    fn is_finished(&self) -> bool {
        // NOTE: The finished should always be matching since they have the same duration.
        self.x.is_finished() /* || self.y.is_finished() */
    }

    /// Get the latest value calculated from [`Self::tick`]
    fn value(&self) -> Point<i32, Logical> {
        (*self.x.value(), *self.y.value()).into()
    }
}
