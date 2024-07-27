//! A single workspace tile.
//!
//! This is an abstraction over an element that implements [`WorkspaceElement`]. For more
//! information, check [the `workspaces` module documentation](crate::shell::workspaces)

use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::utils::{
    Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::{Element, Id, Kind};
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::Renderer;
use smithay::desktop::space::SpaceElement;
use smithay::desktop::{PopupManager, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{
    IsAlive, Logical, Monotonic, Physical, Point, Rectangle, Scale, Size, Time, Transform,
};
use smithay::wayland::compositor::{with_surface_tree_downward, TraversalAction};
use smithay::wayland::seat::WaylandFocus;

use crate::config::{BorderConfig, CONFIG};
use crate::egui::{EguiElement, EguiRenderElement};
use crate::renderer::extra_damage::ExtraDamage;
use crate::renderer::pixel_shader_element::FhtPixelShaderElement;
use crate::renderer::rounded_element::RoundedCornerElement;
use crate::renderer::rounded_outline_shader::{RoundedOutlineElement, RoundedOutlineSettings};
use crate::renderer::texture_element::FhtTextureElement;
use crate::renderer::{render_to_texture, FhtRenderer};
use crate::utils::animation::Animation;
use crate::utils::RectCenterExt;

// I did not finish implementing everything using this trait.
//
// TODO: Maybe remove some of the trait requirements? I should keep this trait very "abstract" so
// that I can technically render anything inside.
#[allow(unused)]
pub trait WorkspaceElement:
    Clone + std::fmt::Debug + SpaceElement + WaylandFocus + IsAlive + Sized + PartialEq
{
    /// Send a configure message to this element.
    ///
    /// Wayland works by accumulating changes between commits and then when either the XDG toplevel
    /// window or the server/compositor send a configure message, the changes are then applied.
    fn send_pending_configure(&self);

    /// Set the size of this element.
    ///
    /// The element should not send a configure message with this.
    fn set_size(&self, new_size: Size<i32, Logical>);
    /// Get the size of this element.
    fn size(&self) -> Size<i32, Logical>;

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
    fn set_bounds(&self, bounds: Option<Size<i32, Logical>>);
    /// Get the bounds of this element.
    fn bounds(&self) -> Option<Size<i32, Logical>>;

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

    /// Render the surface elements.
    ///
    /// It is up to the trait implementation to actually offset the render elements to match the
    /// given `location`, if applicable.
    fn render_surface_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<WaylandSurfaceRenderElement<R>>;

    /// Render the popup elements.
    ///
    /// It is up to the trait implementation to actually offset the render elements to match the
    /// given `location`, if applicable.
    fn render_popup_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<WaylandSurfaceRenderElement<R>>;

    /// Set the offscreen element id.
    ///
    /// Sometimes we need to render inside a [`GlesTexture`] for animation purposes.
    fn set_offscreen_element_id(&self, id: Option<Id>);
    /// Get the offscreen element id.
    fn get_offscreen_element_id(&self) -> Option<Id>;
}

/// A single [`Workspace`] tile.
///
/// A [`WorkspaceTile`] is responsible for managing an inner [`WorkspaceElement`] by giving a
/// position, border, and other properties. This tile is useful only if you store it inside a
/// [`Workspace`](super::Workspace)
pub struct WorkspaceTile<E: WorkspaceElement> {
    /// The inner element.
    pub(crate) element: E,

    /// The location of this tile, relative to the [`Workspace`] that holds it.
    ///
    /// This location should be the top left corner of the tile's element, in other terms excluding
    /// the client-side decorations
    pub location: Point<i32, Logical>,

    /// The currently client fact added to this tile.
    ///
    /// This float being higher means that this tile of the [`Workspace`] will take more or less
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
    pub temporary_render_location: Option<Point<i32, Logical>>,

    /// Location animation
    ///
    /// This value should be an offset getting closer to zero.
    pub location_animation: Option<Animation<Point<i32, Logical>>>,

    /// Open/Close animation.
    pub open_close_animation: Option<OpenCloseAnimation>,
    /// A snapshot of the last frame before the tile closes.
    ///
    /// Due to a limitation in wayland, we need to prepare the close render elements in advance
    /// before we start the close animation, since the window will have the buffers unmapped
    /// or destroyed by then.
    ///
    /// It is up to the parent compositor to decide how to handle this.
    close_animation_snapshot: Option<Vec<WorkspaceTileRenderElement<GlowRenderer>>>,

    /// The egui debug overlay for this element.
    pub debug_overlay: Option<EguiElement>,
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
        let element_size = element.size();

        Self {
            element,
            location: Point::default(),
            cfact: 1.0,
            border_config,
            rounded_corner_damage: ExtraDamage::default(),
            temporary_render_location: None,
            location_animation: None,
            open_close_animation: None,
            close_animation_snapshot: None,
            debug_overlay: CONFIG
                .renderer
                .tile_debug_overlay
                .then(|| EguiElement::new(element_size)),
        }
    }

    /// Get a reference to this tile's inner element.
    pub fn element(&self) -> &E {
        &self.element
    }

    /// Set this tile's geometry, relative to the [`Workspace`] that holds it.
    ///
    /// `new_geo` is assumed to be the the tile's visual geometry, excluding client side decorations
    /// like shadows.
    pub fn set_geometry(&mut self, mut new_geo: Rectangle<i32, Logical>, animate: bool) {
        let thickness = self.border_config().thickness as i32;
        if thickness > 0 {
            let thickness = self.border_config().thickness as i32;
            new_geo.loc += (thickness, thickness).into();
            new_geo.size -= (2 * thickness, 2 * thickness).into();
        }

        self.element.set_size(new_geo.size);
        self.element.send_pending_configure();
        self.rounded_corner_damage.set_size(new_geo.size);
        if let Some(egui) = self.debug_overlay.as_mut() {
            egui.set_size(new_geo.size);
        }

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

    /// Send a pending configure message to the element.
    pub fn send_pending_configure(&mut self) {
        self.element.send_pending_configure();
    }

    /// Start this tile's opening animation.
    pub fn start_opening_animation(&mut self) {
        let Some(progress) = Animation::new(
            0.0,
            1.0,
            CONFIG.animation.window_open_close.curve,
            Duration::from_millis(CONFIG.animation.window_open_close.duration),
        ) else {
            return;
        };

        self.open_close_animation = Some(OpenCloseAnimation::Opening { progress })
    }

    /// Prepare a close animation render elements.
    pub fn prepare_close_animation(&mut self, renderer: &mut GlowRenderer, scale: Scale<f64>) {
        if self.close_animation_snapshot.is_some() {
            return;
        }

        // NOTE: We use the border thickness as the location to actually include
        // it with the render elements, otherwise it
        // would be clipped out of the tile.
        let thickness = self.border_config().thickness as i32;
        let border_offset = Point::<i32, Logical>::from((thickness, thickness))
            .to_physical_precise_round::<_, i32>(scale);
        let elements = self
            .render_elements_inner(
                renderer,
                border_offset,
                scale,
                1.0,
                true, // TODO: Maybe maybe not, this is just a detail
            )
            .collect::<Vec<_>>();
        self.close_animation_snapshot = Some(elements);
    }

    /// Prepare a close animation render elements.
    pub fn clear_close_snapshot(&mut self) {
        let _ = self.close_animation_snapshot.take();
    }

    /// Start the closing animation.
    ///
    /// Having a `renderer` passed is mandatory for us to store the last window frame.
    pub fn start_close_animation(&mut self, renderer: &mut GlowRenderer, scale: Scale<f64>) {
        let Some(elements) = self.close_animation_snapshot.take() else {
            return;
        };
        let thickness = self.border_config().thickness as i32;
        let tile_size = self.element.size() + (thickness * 2, thickness * 2).into();

        let Some(progress) = Animation::new(
            1.0,
            0.0,
            CONFIG.animation.window_open_close.curve,
            Duration::from_millis(CONFIG.animation.window_open_close.duration),
        ) else {
            return;
        };

        let geo = elements
            .iter()
            .fold(Rectangle::default(), |acc, e| acc.merge(e.geometry(scale)));
        let elements = elements.into_iter().rev().map(|e| {
            RelocateRenderElement::from_element(e, (-geo.loc.x, -geo.loc.y), Relocate::Relative)
        });

        let Ok(texture) = render_to_texture(
            renderer,
            geo.size,
            scale,
            Transform::Normal,
            Fourcc::Abgr8888,
            elements.into_iter(),
        )
        .map(|(tex, _)| tex)
        .map_err(|err| warn!(?err, "Failed to render to texture for close animation")) else {
            return;
        };

        let texture = TextureBuffer::from_texture(
            renderer,
            texture,
            scale.x.max(scale.y) as i32,
            Transform::Normal,
            None,
        );
        let offset = geo.loc.to_f64().to_logical(scale).to_i32_round();

        self.open_close_animation = Some(OpenCloseAnimation::Closing {
            texture,
            offset,
            tile_size,
            progress,
        });
    }

    /// Get this tile's geometry, IE the topleft point of the tile's visual geometry, excluding
    /// client side decorations like shadows, relative to the [`Workspace`] that holds it
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        let mut geo = self.element.geometry();
        geo.loc = self.location;
        geo
    }

    /// Get this tile's visual geometry, IE the topleft point of the tile's visual geometry,
    /// excluding client side decorations like shadows, relative to the [`Workspace`] that holds it.
    pub fn visual_geometry(&self) -> Rectangle<i32, Logical> {
        let mut geo = self.element.geometry();
        geo.loc = self.render_location();
        geo
    }

    /// Get this tile's bounding box, relative to the [`Workspace`] that holds it.
    pub fn bbox(&self) -> Rectangle<i32, Logical> {
        let mut bbox = self.element.bbox();
        bbox.loc = self.location;
        bbox
    }

    /// Get this tile's render location, IE the topleft point of the tile's visual geometry,
    /// excluding client side decorations like shadows, relative to the [`Workspace`] that holds it.
    pub fn render_location(&self) -> Point<i32, Logical> {
        let mut render_location = self.temporary_render_location.unwrap_or(self.location);
        if let Some(offset) = self.location_animation.as_ref().map(Animation::value) {
            render_location += offset;
        }

        render_location
    }

    /// Return the border settings to use when rendering this tile.
    pub fn border_config(&self) -> BorderConfig {
        self.border_config.unwrap_or(CONFIG.decoration.border)
    }

    /// Advance this tile's animations.
    pub fn advance_animations(&mut self, current_time: Time<Monotonic>) -> bool {
        let mut ret = false;

        let _ = self.location_animation.take_if(|anim| anim.is_finished());
        if let Some(location_animation) = self.location_animation.as_mut() {
            location_animation.set_current_time(current_time);
            ret |= true;
        }

        let _ = self.open_close_animation.take_if(|anim| anim.is_finished());
        if let Some(open_close_animation) = self.open_close_animation.as_mut() {
            open_close_animation.set_current_time(current_time);
            ret |= true;
        }

        ret
    }

    /// Return whether this tile contains this [`WlSurface`] of [`WindowSurfaceType`]
    pub fn has_surface(&self, surface: &WlSurface, surface_type: WindowSurfaceType) -> bool {
        let Some(element_surface) = self.element.wl_surface() else {
            return false;
        };

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

    /// Draw an egui overlay for this tile.
    fn egui_overlay(&self, ctx: &egui::Context) {
        egui::Area::new("tile-debug-overlay")
            .fixed_pos((0.0, 0.0))
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(egui::Color32::from_black_alpha((255 / 3) * 2))
                    .inner_margin(8.0)
                    .outer_margin(8.0)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = Default::default();
                        let info = |ui: &mut egui::Ui, name: &str, value: &str| {
                            ui.horizontal_wrapped(|ui| {
                                ui.style_mut().spacing.item_spacing.x = 0.0;
                                ui.label(name);
                                ui.label(": ");
                                ui.monospace(value);
                            });
                        };

                        ui.label("Window info");
                        ui.indent("Window info", |ui| {
                            info(ui, "title", self.element.title().as_str());
                            info(ui, "app-id", self.element.app_id().as_str());
                        });

                        ui.add_space(4.0);

                        ui.label("Window geometry");
                        ui.indent("Window geometry", |ui| {
                            info(ui, "location", {
                                let location = self.location;
                                format!("({}, {})", location.x, location.y).as_str()
                            });
                            info(ui, "size", {
                                let size = self.element.size();
                                format!("({}, {})", size.w, size.h).as_str()
                            });
                            info(ui, "cfact", self.cfact.to_string().as_str());
                            info(ui, "render-location", {
                                let location = self.render_location();
                                format!("({}, {})", location.x, location.y).as_str()
                            });
                        });

                        ui.add_space(4.0);

                        ui.label("Window state");
                        ui.indent("XDG toplevel state", |ui| {
                            info(
                                ui,
                                "fullscreen",
                                self.element.fullscreen().to_string().as_str(),
                            );
                            info(
                                ui,
                                "maximized",
                                self.element.maximized().to_string().as_str(),
                            );
                            info(
                                ui,
                                "bounds",
                                self.element
                                    .bounds()
                                    .map(|bounds| format!("({}, {})", bounds.w, bounds.h))
                                    .unwrap_or_else(|| String::from("None"))
                                    .as_str(),
                            )
                        });

                        ui.add_space(4.0);

                        ui.label("Open-close animation");
                        ui.indent("Open-close animation", |ui| {
                            if let Some(anim) = self.open_close_animation.as_ref() {
                                info(
                                    ui,
                                    "Kind",
                                    if matches!(anim, OpenCloseAnimation::Opening { .. }) {
                                        "opening"
                                    } else {
                                        "closing"
                                    },
                                );
                                let alpha = anim.alpha();
                                info(ui, "Alpha progress", format!("{:.3}", alpha).as_str());

                                let scale = anim.scale();
                                info(ui, "Scale progress", format!("{:.3}", scale).as_str());
                            } else {
                                ui.label("Not running");
                            }
                        })
                    });
            });
    }

    /// Generate the render elements for this tile.
    fn render_elements_inner<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
        focused: bool,
    ) -> impl Iterator<Item = WorkspaceTileRenderElement<R>> {
        // The tile's physical geometry, as in where our render elements will be when drawn
        let physical_geo = Rectangle::from_loc_and_size(
            location,
            self.element.size().to_physical_precise_round(scale),
        );
        // The tile geometry in compositor space, IE what the user sees as being the window.
        let tile_geo = physical_geo.to_f64().to_logical(scale).to_i32_round();

        let border_config = self.border_config.unwrap_or(CONFIG.decoration.border);
        let need_border = !self.element.fullscreen();
        let radius = border_config.radius();

        let window_elements = self
            .element
            .render_surface_elements(renderer, physical_geo.loc, scale, alpha)
            .into_iter()
            .map(move |e| {
                if !need_border {
                    return WorkspaceTileRenderElement::Element(e);
                }

                // Rounding off windows is a little tricky.
                //
                // Not every surface of the window means its "the window", not at all.
                // Some clients (like OBS-studio) use subsurfaces (not popups) to display different
                // parts of their interface (for example OBs does this with the preview window)
                //
                // To counter this, we check here if the surface is going to clip.
                if RoundedCornerElement::will_clip(&e, scale, tile_geo, radius) {
                    let rounded = RoundedCornerElement::new(e, radius, tile_geo, scale);
                    WorkspaceTileRenderElement::RoundedElement(rounded)
                } else {
                    WorkspaceTileRenderElement::Element(e)
                }
            });
        let popup_elements = self
            .element
            .render_popup_elements(renderer, physical_geo.loc, scale, alpha)
            .into_iter()
            .map(WorkspaceTileRenderElement::Element);

        // We need to have extra damage in the case we have a radius ontop of our window
        let damage = (radius != 0.0)
            .then(|| {
                let damage = self
                    .rounded_corner_damage
                    .clone()
                    .with_location(tile_geo.loc);
                WorkspaceTileRenderElement::RoundedElementDamage(damage)
            })
            .into_iter();

        // Same deal for the border, only if the thickness is non-null
        let border_element = (border_config.thickness != 0)
            .then(|| {
                let mut border_geo = tile_geo;
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

        popup_elements
            .into_iter()
            .chain(damage)
            .chain(window_elements)
            .chain(border_element)
    }

    pub fn render_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        output: &Output,
        scale: Scale<f64>,
        alpha: f32,
        focused: bool,
    ) -> impl Iterator<Item = WorkspaceTileRenderElement<R>> {
        let mut render_geo = self.visual_geometry().to_physical_precise_round(scale);

        let debug_overlay = self
            .debug_overlay
            .as_ref()
            .map(|egui| {
                // TODO: Maybe use smithay's clock? But it just does this under the hood soo.
                use smithay::reexports::rustix;
                let time = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
                let time = Duration::new(time.tv_sec as u64, time.tv_nsec as u32);
                let element = egui
                    .render(
                        renderer.glow_renderer_mut(),
                        scale.x as i32,
                        alpha,
                        render_geo.loc,
                        |ctx| self.egui_overlay(ctx),
                        time,
                    )
                    .unwrap();
                WorkspaceTileRenderElement::DebugOverlay(element)
            })
            .into_iter();

        let mut opening_elements = None;
        let mut closing_elements = None;
        let mut normal_elements = None;

        // NOTE: We need to offset the render elements by -(thickness,thickness) and
        // +(thickness,thickness) when generating the texture is because of how we draw the border
        // around the tile
        //
        // It is expected by render_elements_inner that `location` is the border corner of the
        // window. If the border is drawn, its location will be `location - (thickness, thickness)`
        //
        // So, to actually include the border inside the texture, we render the window at
        // (thickness, thickness), then the texture render elements get offset back.
        let thickness = self.border_config().thickness as i32;
        let border_offset = Point::<i32, Logical>::from((thickness, thickness))
            .to_physical_precise_round::<_, i32>(scale);

        if let Some(OpenCloseAnimation::Opening { progress }) = self.open_close_animation.as_ref() {
            let progress = progress.value();

            let glow_renderer = renderer.glow_renderer_mut();
            // NOTE: We use the border thickness as the location to actually include it with the
            // render elements, otherwise it would be clipped out of the tile.
            let elements = self
                .render_elements_inner(glow_renderer, border_offset, scale, alpha, focused)
                .collect::<Vec<_>>();
            let rec = elements
                .iter()
                .fold(Rectangle::default(), |acc, e| acc.merge(e.geometry(scale)));

            opening_elements = Some(
                render_to_texture(
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
                    render_geo.loc -= border_offset;
                    render_geo.size += border_offset.to_size().upscale(2);

                    let element_id = Id::new();
                    let texture: FhtTextureElement = TextureRenderElement::from_static_texture(
                        element_id.clone(),
                        glow_renderer.id(),
                        render_geo.loc.to_f64(),
                        texture,
                        scale.x.max(scale.y) as i32,
                        Transform::Normal,
                        Some(progress.clamp(0., 1.) as f32),
                        None,
                        None,
                        None,
                        Kind::Unspecified,
                    )
                    .into();
                    self.element.set_offscreen_element_id(Some(element_id));

                    let origin = render_geo.center();
                    let rescale = (progress * (1.0 - OpenCloseAnimation::OPEN_SCALE_THRESHOLD))
                        + OpenCloseAnimation::OPEN_SCALE_THRESHOLD;
                    let rescale = RescaleRenderElement::from_element(texture, origin, rescale);

                    WorkspaceTileRenderElement::<R>::OpenClose(
                        WorkspaceTileOpenCloseElement::OpenTexture(rescale),
                    )
                })
                .into_iter(),
            )
        };

        if let Some(OpenCloseAnimation::Closing {
            texture,
            offset,
            tile_size,
            progress,
        }) = self.open_close_animation.as_ref()
        {
            let texture = texture.clone();
            let progress = progress.value();

            let texture: FhtTextureElement = TextureRenderElement::from_texture_buffer(
                Point::from((0., 0.)),
                &texture,
                Some(progress.clamp(0., 1.) as f32),
                None,
                None,
                Kind::Unspecified,
            )
            .into();

            let offset = *offset;
            let center = (*tile_size).to_point().downscale(2);
            let origin = (center + offset).to_physical_precise_round(scale);
            let rescale = progress * (1.0 - OpenCloseAnimation::CLOSE_SCALE_THRESHOLD)
                + OpenCloseAnimation::CLOSE_SCALE_THRESHOLD;
            let rescale = RescaleRenderElement::from_element(texture, origin, rescale);

            let location = render_geo.loc + offset.to_physical_precise_round(scale);
            let relocate =
                RelocateRenderElement::from_element(rescale, location, Relocate::Relative);

            let element = WorkspaceTileRenderElement::<R>::OpenClose(
                WorkspaceTileOpenCloseElement::CloseTexture(relocate),
            );

            closing_elements = Some(Some(element).into_iter())
        };

        if opening_elements.is_none() && closing_elements.is_none() {
            self.element.set_offscreen_element_id(None);
            normal_elements =
                Some(self.render_elements_inner(renderer, render_geo.loc, scale, alpha, focused))
        }

        debug_overlay
            .chain(opening_elements.into_iter().flatten())
            .chain(closing_elements.into_iter().flatten())
            .chain(normal_elements.into_iter().flatten())
    }
}

impl<E: WorkspaceElement> IsAlive for WorkspaceTile<E> {
    fn alive(&self) -> bool {
        if matches!(
            &self.open_close_animation,
            Some(anim) if !anim.is_finished()
        ) {
            // We do not want to clear our the window if we opening/closing
            return true;
        }
        self.element.alive()
    }
}

crate::fht_render_elements! {
    WorkspaceTileRenderElement<R> => {
        Element = WaylandSurfaceRenderElement<R>,
        RoundedElement = RoundedCornerElement<WaylandSurfaceRenderElement<R>>,
        RoundedElementDamage = ExtraDamage,
        Border = FhtPixelShaderElement,
        DebugOverlay = EguiRenderElement,
        OpenClose = WorkspaceTileOpenCloseElement,
    }
}

crate::fht_render_elements! {
    WorkspaceTileOpenCloseElement => {
        OpenTexture = RescaleRenderElement<FhtTextureElement>,
        // NOTE: After smashing my head very very long on the wall, I found this trick done by niri:
        //
        // to actual position the texture correctly. You first need to render the actual texture at
        // (0,0), then rescale it, then use the relocate render element to actually position it.
        CloseTexture = RelocateRenderElement<RescaleRenderElement<FhtTextureElement>>,
    }
}

pub enum OpenCloseAnimation {
    Opening {
        progress: Animation,
    },
    Closing {
        // For closing animation, we need to keep a last render of the window before closing, so
        // that we can render it even after it dies.
        texture: TextureBuffer<GlesTexture>,
        offset: Point<i32, Logical>,
        tile_size: Size<i32, Logical>,
        progress: Animation,
    },
}

impl OpenCloseAnimation {
    // We dont display the window directly, we instead have thresholds of scale where we start
    // animating the window in using the alpha, then we scale it up.
    const OPEN_SCALE_THRESHOLD: f64 = 0.5;
    const CLOSE_SCALE_THRESHOLD: f64 = 0.8;

    fn set_current_time(&mut self, new_current_time: Time<Monotonic>) {
        match self {
            Self::Opening { progress } | Self::Closing { progress, .. } => {
                progress.set_current_time(new_current_time);
            }
        }
    }

    fn is_finished(&self) -> bool {
        match self {
            Self::Opening { progress } => progress.is_finished(),
            Self::Closing { progress, .. } => {
                // If we are 0, then byebye.
                let value = progress.value();
                let value = (value * (1.0 - Self::CLOSE_SCALE_THRESHOLD)).max(0.0);
                value <= 1.0e-3 // since it never reaches 0 really.
            }
        }
    }

    fn scale(&self) -> f64 {
        match self {
            Self::Opening { progress } => {
                let value = progress.value();
                value * (1.0 - Self::OPEN_SCALE_THRESHOLD) + Self::OPEN_SCALE_THRESHOLD
            }
            Self::Closing { progress, .. } => {
                let value = progress.value();
                value * (1.0 - Self::CLOSE_SCALE_THRESHOLD) + Self::CLOSE_SCALE_THRESHOLD
            }
        }
    }

    fn alpha(&self) -> f32 {
        match self {
            Self::Opening { progress } | Self::Closing { progress, .. } => progress.value() as f32,
        }
    }
}
