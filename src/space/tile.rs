use std::rc::Rc;
use std::time::Duration;

use fht_animation::Animation;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind};
use smithay::backend::renderer::gles::element::TextureShaderElement;
use smithay::backend::renderer::gles::Uniform;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::Renderer as _;
use smithay::desktop::{PopupManager, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::compositor::{with_surface_tree_downward, TraversalAction};
use smithay::wayland::seat::WaylandFocus;

use super::border::Border;
use super::closing_tile::ClosingTile;
use super::{border, shadow, Config};
use crate::egui::EguiRenderElement;
use crate::output::OutputExt;
use crate::renderer::blur::element::BlurElement;
use crate::renderer::extra_damage::ExtraDamage;
use crate::renderer::rounded_window::RoundedWindowElement;
use crate::renderer::shaders::{ShaderElement, Shaders};
use crate::renderer::texture_element::FhtTextureElement;
use crate::renderer::texture_shader_element::FhtTextureShaderElement;
use crate::renderer::{has_transparent_region, render_to_texture, FhtRenderer};
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
    location_animation: Option<Animation<[i32; 2]>>,

    /// The current size animation of this tile.
    ///
    /// The animation value's (if any) is the visual size we should display the [`Tile`] with.
    size_animation: Option<Animation<[i32; 2]>>,

    /// The border around this tile.
    border: Border,

    /// The box shadow around this tile.
    shadow: shadow::Shadow,

    /// Extra damage bag to apply when the tile corners are being rounded.
    /// This is due to an implementation detail of [`RoundedWindowElement`]
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
        RoundedSurface = RoundedWindowElement<R>,
        RoundedSurfaceDamage = ExtraDamage,
        ResizingSurface = FhtTextureShaderElement,
        Decoration = ShaderElement,
        Blur = BlurElement,
        DebugOverlay = EguiRenderElement,
        Opening = RescaleRenderElement<FhtTextureElement>,
    }
}

impl Tile {
    /// Create a new [`Tile`] from this window.
    pub fn new(window: Window, config: Rc<Config>) -> Self {
        let size = window.size();
        let rules = window.rules();
        let proportion = rules.proportion.unwrap_or(1.0);

        // Compute initial border state
        let border = config.border.with_overrides(&rules.border);
        let shadow = config.shadow.with_overrides(&rules.shadow);
        let border_parameters = super::border::Parameters {
            color: border.normal_color,
            corner_radius: border.radius,
            thickness: border.thickness,
        };
        let shadow_parameters = shadow::Parameters {
            disable: shadow.disable,
            floating_only: shadow.floating_only,
            blur_sigma: shadow.sigma,
            corner_radius: border.radius,
            color: shadow.color,
        };
        let border_geometry = Rectangle::from_size(Size::from((
            size.w + border_parameters.thickness * 2,
            size.h + border_parameters.thickness * 2,
        )));

        drop(rules);

        Self {
            window,
            location: Point::default(),
            proportion,
            location_animation: None,
            size_animation: None,
            opening_animation: None,
            border: Border::new(
                border_geometry,
                border_parameters,
                config.border_animation.as_ref(),
            ),
            shadow: shadow::Shadow::new(border_geometry, shadow_parameters),
            extra_damage: ExtraDamage::new(size),
            close_animation_snapshot: None,
            config,
        }
    }

    /// Refresh the internal state of this [`Tile`].
    pub fn refresh(&mut self, output: &Output, active: bool) {
        crate::profile_function!();
        let output_geometry = output.geometry();
        let window = &self.window;
        window.request_activated(active);

        let mut bbox = window.bbox();
        bbox.loc = self.location() + self.window_loc() + output_geometry.loc;
        if let Some(mut overlap) = output_geometry.intersection(bbox) {
            // overlap must be in window-local coordinates.
            overlap.loc -= bbox.loc;
            window.enter_output(output, overlap);
        }

        window.send_pending_configure();
        window.refresh();

        // Update the decorations too.
        self.update_decorations(active);
    }

    /// Update the configuration of this [`Tile`]
    pub fn update_config(&mut self, config: Rc<Config>) {
        self.config = config;
        self.border
            .update_config(self.config.border_animation.as_ref());
    }

    fn update_decorations(&mut self, active: bool) {
        // FIXME: This might not be always accurate, to be exact, window rules must be
        // refreshed inside Tile::refresh, but this could be really expensive
        let rules = self.window.rules();
        let mut border = self.config.border.with_overrides(&rules.border);
        let mut shadow = self.config.shadow.with_overrides(&rules.shadow);
        drop(rules);

        let is_fullscreen = self.window.fullscreen();

        if is_fullscreen {
            // Disable border for fullscreened windos
            border.radius = 0.0;
            shadow.disable = true;
        }

        self.border.update_parameters(super::border::Parameters {
            color: if active {
                border.focused_color
            } else {
                border.normal_color
            },
            corner_radius: border.radius,
            thickness: border.thickness,
        });
        let border::Parameters {
            thickness: border_thickness,
            corner_radius,
            ..
        } = self.border.current_parameters();
        self.shadow.update_parameters(shadow::Parameters {
            disable: shadow.disable,
            floating_only: shadow.floating_only,
            blur_sigma: shadow.sigma,
            corner_radius,
            color: shadow.color,
        });
        // Keep the geometry updated, though if there's a mismatch the render function will handle
        // that for us (in the case of a opening animation, for example)
        let visual_geometry = self.visual_geometry();
        let window_geometry = Rectangle::new(
            visual_geometry.loc - Point::from((border_thickness, border_thickness)),
            visual_geometry.size + Size::from((border_thickness, border_thickness)).upscale(2),
        );
        self.border.set_geometry(visual_geometry);
        self.shadow.set_geometry(window_geometry);
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
                window_surface,
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
            return PopupManager::popups_for_surface(window_surface)
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

    /// Get this [`Tile`]'s geometry, in other words its effective [`Rectangle`] in [`Workspace`]
    /// space.
    ///
    /// The returned [`Rectangle`] is the geometry of the whole [`Tile`], including its border.
    ///
    /// This accounts for any ongoing location animation.
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(self.location, self.size())
    }

    /// Get this [`Tile`]'s visual geometry, in other words where the [`Tile`]'s [`Rectangle`] will
    /// end up at when rendering.
    ///
    /// The returned [`Rectangle`] is the geometry of the whole [`Tile`], including its border.
    ///
    /// This accounts for ongoing animations.
    pub fn visual_geometry(&self) -> Rectangle<i32, Logical> {
        let mut rect = Rectangle::new(self.visual_location(), self.visual_size());
        if let Some(opening_animation) = self.opening_animation.as_ref() {
            let progress = *opening_animation.value();
            let scale = opening_animation_progress_to_scale(progress);
            let center = rect.center();

            rect.loc -= center;
            rect = rect.to_f64().upscale(scale).to_i32_round();
            rect.loc += center;
        }

        rect
    }

    /// Set this [`Tile`]'s location.
    ///
    /// The `new_location` argument will the location of the whole [`Tile`], including its border.
    pub fn set_location(&mut self, new_location: Point<i32, Logical>, animate: bool) {
        if let Some(window_geometry_animation) = &self.config.window_geometry_animation {
            let mut old_location = self.location;
            if let Some(previous_animation) = self.location_animation.take() {
                let [x, y] = *previous_animation.value();
                old_location += Point::from((x, y));
            }
            self.location = new_location;
            if animate {
                let (delta_x, delta_y) = (old_location - new_location).into();
                self.location_animation = Some(
                    Animation::new(
                        [delta_x, delta_y],
                        [0, 0],
                        window_geometry_animation.duration,
                    )
                    .with_curve(window_geometry_animation.curve),
                );
            }
        } else if self.location != new_location {
            self.location = new_location
        }
    }

    /// Get this [`Tile`]'s visual location, in orther words where the tile is going to **render**.
    ///
    /// The returned value will the location of the whole [`Tile`], including its border.
    pub fn visual_location(&self) -> Point<i32, Logical> {
        let mut tile_location = self.location;
        if let Some(animation) = &self.location_animation {
            let [x, y] = *animation.value();
            tile_location += Point::from((x, y));
        }
        tile_location
    }

    /// Get this [`Tile`]'s location, in other words its effective location in [`Workspace`] space.
    ///
    /// The returned value will the size of the whole [`Tile`], including its border.
    pub fn location(&self) -> Point<i32, Logical> {
        self.location
    }

    /// Get this [`Tile`]'s visual size, in other words the size that its going to **render** with.
    ///
    /// The returned value will the size of the whole [`Tile`], including its border.
    pub fn visual_size(&self) -> Size<i32, Logical> {
        self.size_animation
            .as_ref()
            .map(|animation| array_to_size(*animation.value()))
            .unwrap_or_else(|| self.size())
    }

    /// Get this [`Tile`]'s size, in other words its effective size in [`Workspace`] space.
    ///
    /// The returned value will the size of the whole [`Tile`], including its border.
    pub fn size(&self) -> Size<i32, Logical> {
        let Size { w: ww, h: wh, .. } = self.window.size();
        let border_thickness = self.border.parameters().thickness;
        Size::from((ww + 2 * border_thickness, wh + 2 * border_thickness))
    }

    /// Get the [`Window`]'s location relative to this [`Tile`].
    ///
    /// A [`Tile`] can have a border around it, so the actual window will get rendered inside the
    /// border, and not at `self.location`.
    pub fn window_loc(&self) -> Point<i32, Logical> {
        let thickness = self.border.parameters().thickness;
        Point::from((thickness, thickness))
    }

    /// Set this [`Tile`]'s size.
    ///
    /// The `new_size` argument will the size of the whole [`Tile`], including its border.
    ///
    /// This does not call [`Window::send_configure`]!
    pub fn set_size(&mut self, new_size: Size<i32, Logical>, animate: bool) {
        let previous_size = self.visual_size(); // we need visual for animation
        if previous_size == new_size
            || self.size_animation.as_ref().is_some_and(|anim| {
                // dont reanimate if we are resizing to the same target.
                array_to_size(anim.end) == new_size
            })
        {
            return;
        }

        self.extra_damage.set_size(new_size);
        let mut border_thickness = self.border.parameters().thickness;
        if self.window.fullscreen() {
            // When we have a fullscreen window, no border is drawn
            border_thickness = 0;
        }
        self.window.request_size(Size::from((
            new_size.w - 2 * border_thickness,
            new_size.h - 2 * border_thickness,
        )));

        if animate {
            // When we request a size change, we wait till the window buffers are resized and the
            // window has drawn at least one frame with the new size (generally on the
            // next commit cycle).
            //
            // When we start rendering with that frame, we use a custom texture shader in order to
            // draw it, instead of the included smithay one, in order to stretch/crop
            // the window contents and apply proper corner rounding.
            let prev = self
                .size_animation
                .take()
                .map(|animation| array_to_size(*animation.value()))
                .unwrap_or(previous_size);
            if let Some(config) = self.config.window_geometry_animation.as_ref() {
                self.size_animation = Some(
                    Animation::new([prev.w, prev.h], [new_size.w, new_size.h], config.duration)
                        .with_curve(config.curve),
                );
            }
        }
    }

    /// Return whether this tile has a transparent region.
    pub fn has_transparent_region(&self) -> bool {
        let wl_surface = self
            .window()
            .wl_surface()
            .expect("A mapped window should have a WlSurface");
        // We only care about main window surface blurring, subsurfaces (for example
        // popups) are not accoutned for and will not be rendered
        // with blur.
        has_transparent_region(&wl_surface, self.window.size())
    }

    /// Advance animations for this [`Tile`].
    pub fn advance_animations(&mut self, target_presentation_time: Duration) -> bool {
        crate::profile_function!();
        let mut animations_ongoing = false;

        let _ = self.location_animation.take_if(|a| a.is_finished());
        if let Some(animation) = &mut self.location_animation {
            animations_ongoing = true;
            animation.tick(target_presentation_time);
        }

        let _ = self.size_animation.take_if(|a| a.is_finished());
        if let Some(animation) = &mut self.size_animation {
            animations_ongoing = true;
            animation.tick(target_presentation_time);
        }

        let _ = self.opening_animation.take_if(|a| a.is_finished());
        if let Some(animation) = &mut self.opening_animation {
            animations_ongoing = true;
            animation.tick(target_presentation_time);
        }

        if self.border.advance_animations(target_presentation_time) {
            animations_ongoing = true;
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
    pub fn prepare_close_animation_if_needed(&mut self, renderer: &mut GlowRenderer, scale: i32) {
        if self.close_animation_snapshot.is_some() {
            return;
        }

        // FIXME: Blur with closing tiles is kinda wonky, but you shouldn't notice it unless
        // you have a *very* slow closing animation
        let elements = self.render_inner(renderer, (0, 0).into(), scale, 1.0);
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
        scale: i32,
        alpha: f32,
    ) -> Vec<TileRenderElement<R>> {
        crate::profile_function!();
        let mut elements = vec![];
        let is_fullscreen = self.window.fullscreen();
        let rules = self.window.rules();

        let alpha = if is_fullscreen {
            alpha
        } else {
            alpha * rules.opacity.unwrap_or(1.0)
        };

        let border::Parameters {
            thickness: mut border_thickness,
            corner_radius: border_radius,
            ..
        } = self.border.current_parameters();
        if is_fullscreen {
            border_thickness = 0
        }

        drop(rules); // Avoid deadlock :skull:

        let has_size_animation = self.size_animation.is_some();
        let tile_geometry = Rectangle::new(location, self.visual_size());
        let window_geometry = Rectangle::new(
            location + Point::<i32, Logical>::from((border_thickness, border_thickness)),
            (
                tile_geometry.size.w - 2 * border_thickness,
                tile_geometry.size.h - 2 * border_thickness,
            )
                .into(),
        );

        // Use the inner radius here
        let border_radius = (border_radius - border_thickness as f32).max(0.0);
        let border_radius = border::fit_corner_radius_to_geometry(tile_geometry, border_radius);

        if border_radius != 0.0 {
            let damage = self.extra_damage.clone().with_location(window_geometry.loc);
            elements.push(TileRenderElement::RoundedSurfaceDamage(damage));
        }

        let popup_elements = self
            .window
            .render_popup_elements(
                renderer,
                window_geometry.loc.to_physical_precise_round(scale),
                scale as f64,
                alpha,
            )
            .into_iter()
            .map(TileRenderElement::Surface);
        elements.extend(popup_elements);

        if has_size_animation {
            // Render inside GlesTexture, and render
            let renderer = renderer.glow_renderer_mut();
            let window_elements =
                self.window
                    .render_toplevel_elements(renderer, (0, 0).into(), scale as f64, alpha);
            let size_animation = self.size_animation.as_ref().unwrap();

            // dont forget to subtract 2 * border since its for the window
            let curr_size = Size::<_, Logical>::from((
                size_animation.value()[0] - 2 * border_thickness,
                size_animation.value()[1] - 2 * border_thickness,
            ));

            // NOTE: We render the elements inside a texture thats the actual window size
            // Cropping and stretching is done inside the texture shader.
            let win_size = self.window.size();
            if let Ok((tex, _)) = render_to_texture(
                renderer,
                window_geometry.size.to_physical_precise_round(scale),
                scale as f64,
                Transform::Normal,
                Fourcc::Abgr8888,
                window_elements.iter(),
            )
            .inspect_err(|err| warn!("Failed to render to texture for size animation: {err:?}"))
            {
                let element_id = Id::new();
                let tex = TextureRenderElement::from_static_texture(
                    element_id.clone(),
                    renderer.context_id(),
                    window_geometry.loc.to_physical(scale).to_f64(),
                    tex,
                    scale,
                    Transform::Normal,
                    Some(alpha),
                    None,
                    // NOTE: we changed the size to curr_size by now
                    Some(window_geometry.size),
                    None,
                    Kind::Unspecified,
                );

                let program = Shaders::get(renderer).resizing_texture.clone();
                let element = TextureShaderElement::new(
                    tex,
                    program,
                    vec![
                        // FIXME: Why divide by scale to get proper rounding?
                        Uniform::new("corner_radius", border_radius / scale as f32),
                        Uniform::new("win_size", [win_size.w as f32, win_size.h as f32]),
                        Uniform::new("curr_size", [curr_size.w as f32, curr_size.h as f32]),
                    ],
                );

                self.window
                    .set_offscreen_element_id(Some(element.id().clone()));

                elements.push(TileRenderElement::<R>::ResizingSurface(element.into()));
            }
        } else {
            // FIXME: Why divide by scale to get proper rounding?
            let border_radius = border_radius / scale as f32;
            let window_elements = self
                .window
                .render_toplevel_elements(
                    renderer,
                    window_geometry.loc.to_physical_precise_round(scale),
                    scale as f64,
                    alpha,
                )
                .into_iter()
                .map(move |e| {
                    // Rounding off windows is a little tricky.
                    //
                    // Not every surface of the window means its "the window", not at all.
                    // Some clients (like OBS-studio) use subsurfaces (not popups) to display
                    // different parts of their interface (for example OBs does
                    // this with the preview window)
                    //
                    // To counter this, we check here if the surface is going to clip.
                    if RoundedWindowElement::will_clip(
                        &e,
                        scale as f64,
                        window_geometry,
                        border_radius,
                    ) {
                        let rounded = RoundedWindowElement::new(
                            e,
                            border_radius,
                            window_geometry,
                            scale as f64,
                        );
                        TileRenderElement::RoundedSurface(rounded)
                    } else {
                        TileRenderElement::Surface(e)
                    }
                });
            elements.extend(window_elements);
        };

        // elements.extend();

        elements
    }

    /// Render the elements for this [`Tile`].
    ///
    /// The [`Tile`] uses its `location` to render the elements.
    pub fn render<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        scale: i32,
        mut alpha: f32,
        output: &Output,
        render_offset: Point<i32, Logical>,
    ) -> impl Iterator<Item = TileRenderElement<R>> {
        crate::profile_function!();
        let fractional_scale = Scale::from(scale as f64);
        let mut opening_element = None;
        let mut normal_elements = vec![];

        // Rendering tile goes through the following phases.
        //
        // 1. Tile::render_inner renders the tile's window and popups, and applies the resize the
        //    current size_animation if any to the main window surface(s) and rounded corners
        //
        // 2. If there's a resize animation ongoing, we draw the generated render elements into a
        //    texture and draw it with a custom size and rescale it.
        //
        // 3. We draw additional decorations: blur, shadow, border, etc.
        //
        // FIXME: Redundant calculations here and and in render_inner, but compiler should optimize
        // them away (hopefully)? Either way they are not too expensive.

        let tile_geometry = self.visual_geometry();

        if let Some(animation) = self.opening_animation.as_ref() {
            let progress = *animation.value();
            let opening_animation_scale = opening_animation_progress_to_scale(progress);

            let glow_renderer = renderer.glow_renderer_mut();
            let elements = self.render_inner(glow_renderer, Point::default(), scale, alpha);
            let rec = elements.iter().fold(Rectangle::default(), |acc, e| {
                acc.merge(e.geometry(fractional_scale))
            });
            // Only include alpha now to render the inner window with full alpha.
            // The texture will lower the opacity of that, but for the rest we gotta account for it.
            alpha *= (progress as f32).clamp(0., 1.);

            opening_element = render_to_texture(
                glow_renderer,
                rec.size,
                fractional_scale,
                Transform::Normal,
                Fourcc::Abgr8888,
                elements.into_iter().rev(),
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
                    glow_renderer.context_id(),
                    self.visual_location().to_physical(scale).to_f64(),
                    texture,
                    scale,
                    Transform::Normal,
                    Some(alpha),
                    None,
                    None,
                    None,
                    Kind::Unspecified,
                )
                .into();
                self.window.set_offscreen_element_id(Some(element_id));

                let origin = tile_geometry.to_physical(scale).center();
                let rescale =
                    RescaleRenderElement::from_element(texture, origin, opening_animation_scale);

                TileRenderElement::<R>::Opening(rescale)
            });
        } else {
            self.window.set_offscreen_element_id(None);
            normal_elements = self.render_inner(renderer, self.visual_location(), scale, alpha)
        }

        let is_floating = !self.window.tiled();
        let is_fullscreen = self.window.fullscreen();
        let rules = self.window.rules();
        let border::Parameters {
            thickness: mut border_thickness,
            corner_radius: border_radius,
            ..
        } = self.border.current_parameters();
        if is_fullscreen {
            border_thickness = 0
        }
        let (blur, optimized_blur) = (
            self.config.blur.with_overrides(&rules.blur),
            rules.blur.optimized,
        );

        drop(rules);

        let window_geometry = Rectangle::new(
            tile_geometry.loc + Point::<i32, Logical>::from((border_thickness, border_thickness)),
            tile_geometry.size - Size::from((border_thickness, border_thickness)).upscale(2),
        );

        let border_element = self
            .border
            .render(renderer, alpha)
            .map(|border| {
                border
                    .with_location(tile_geometry.loc)
                    .with_size(tile_geometry.size)
            })
            .map(Into::into)
            .into_iter();

        let shadow_element = self
            .shadow
            .render(renderer, alpha, !self.window.tiled())
            .map(|shadow| {
                // If we expand here, we must take into account the fact that the shadow expands
                // outwards, since it spreads.
                let mut expanded = window_geometry;
                let shadow::Parameters { blur_sigma, .. } = *self.shadow.parameters();
                expanded.loc -= Point::from((blur_sigma as i32, blur_sigma as i32));
                expanded.size += Size::from((blur_sigma as i32, blur_sigma as i32)).upscale(2);

                shadow.with_location(expanded.loc).with_size(expanded.size)
            })
            .map(Into::into)
            .into_iter();

        let blur_element = (!blur.disabled() && self.has_transparent_region())
            .then(|| {
                // Optimized blur uses a pre-blurred texture containing background and bottom
                // layer shells. True blur (non-optimized) blurs in real time whatever is behind the
                // window.
                //
                // When a window is tiled, it will most likely only display the background, IE there
                // are no windows behind it, so we win quit a lot of performance when enabling
                // optimized blur here since tiled windows are *huge*
                //
                // Floating windows on the other hand might have other windows below it, so they
                // don't use optimized. They are also (in comparaison) relatively
                // small, so its even better
                //
                // HACK: Since the true blur implementation is quite expensive as of right now, we
                // use optimized blur by default. Unless the user asks for it.
                let optimized = optimized_blur.unwrap_or(true);

                // render_offset is from the workspace, to account for switching animations
                let sample_area =
                    Rectangle::new(window_geometry.loc + render_offset, window_geometry.size);

                BlurElement::new(
                    renderer,
                    output,
                    sample_area,
                    window_geometry.loc.to_physical(scale),
                    border_radius,
                    optimized,
                    scale,
                    alpha,
                    blur,
                )
                .into()
            })
            .into_iter();

        opening_element
            .into_iter()
            .chain(normal_elements)
            .chain(border_element)
            .chain(blur_element)
            .chain(shadow_element)
    }
}

fn opening_animation_progress_to_scale(progress: f64) -> f64 {
    const OPEN_SCALE_THRESHOLD: f64 = 0.5;
    progress * (1.0 - OPEN_SCALE_THRESHOLD) + OPEN_SCALE_THRESHOLD
}

fn array_to_size<N: smithay::utils::Coordinate, Kind>(array: [N; 2]) -> Size<N, Kind> {
    Size::from((array[0], array[1]))
}
