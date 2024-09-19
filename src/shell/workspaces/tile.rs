//! A single workspace tile.

use std::sync::Arc;
use std::time::Duration;

use fht_animation::{Animation, AnimationCurve};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::Renderer;
use smithay::desktop::{PopupManager, WindowSurfaceType};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Transform};
use smithay::wayland::compositor::{with_surface_tree_downward, TraversalAction};
use smithay::wayland::seat::WaylandFocus;

use super::closing_tile::ClosingTile;
use crate::egui::EguiRenderElement;
use crate::renderer::extra_damage::ExtraDamage;
use crate::renderer::pixel_shader_element::FhtPixelShaderElement;
use crate::renderer::rounded_element::RoundedCornerElement;
use crate::renderer::rounded_outline_shader::{RoundedOutlineElement, RoundedOutlineSettings};
use crate::renderer::texture_element::FhtTextureElement;
use crate::renderer::{render_to_texture, FhtRenderer};
use crate::utils::RectCenterExt;
use crate::window::Window;

pub struct Tile {
    window: Window,
    location: Point<i32, Logical>,
    cfact: f32,
    rounded_corner_damage: ExtraDamage,
    location_animation: Option<LocationAnimation>,
    opening_animation: Option<Animation<f64>>,
    // We prepare the close animation snapshot render elements here before running
    // into_closing_tile in our workspace logic code (and compositor code)
    //
    // Sometimes we want to prepare these in advance since a surface could be destroyed/not alive
    // by the time we actually display the ClosingTile
    close_animation_snapshot: Option<Vec<TileRenderElement<GlowRenderer>>>,
    config: Arc<fht_compositor_config::Config>,
    // TODO: Introcude this back
    // debug_overlay: Option<EguiElement>,
}

impl PartialEq for Tile {
    fn eq(&self, other: &Self) -> bool {
        self.window == other.window
    }
}

impl Tile {
    pub fn new(window: Window, config: Arc<fht_compositor_config::Config>) -> Self {
        Self {
            window,
            location: Point::default(),
            cfact: 1.0,
            rounded_corner_damage: ExtraDamage::default(),
            location_animation: None,
            opening_animation: None,
            close_animation_snapshot: None,
            config,
            // TODO: Introduce this back
            // debug_overlay: CONFIG
            //     .renderer
            //     .tile_debug_overlay
            //     .then(|| EguiElement::new(window_size)),
        }
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn cfact(&self) -> f32 {
        self.cfact
    }

    pub fn change_cfact(&mut self, delta: f32) {
        self.cfact += delta;
    }

    pub fn into_window(self) -> Window {
        self.window
    }

    pub fn into_closing_tile(mut self) -> Option<ClosingTile> {
        Some(ClosingTile::new(
            self.close_animation_snapshot.take()?,
            self.location,
            self.window.size(),
            self.config.animations.window_open_close.duration,
            self.config.animations.window_open_close.curve,
        ))
    }

    pub fn send_pending_configure(&mut self) {
        self.window.send_pending_configure();
    }

    pub fn border_thickness(&self) -> Option<i32> {
        if self.window.fullscreen() {
            return None;
        }

        // TODO: When implementing fractional layout, use f64
        let rules = self.window.rules();
        Some(self.config.decorations.border.with_overrides(&rules.border_overrides).thickness as i32)
    }

    pub fn has_surface(&self, surface: &WlSurface, surface_type: WindowSurfaceType) -> bool {
        let Some(window_surface) = self.window.wl_surface() else {
            return false;
        };

        if surface_type.contains(WindowSurfaceType::TOPLEVEL) && &*window_surface == surface {
            return true;
        }

        if surface_type.contains(WindowSurfaceType::SUBSURFACE) {
            use std::sync::atomic::{AtomicBool, Ordering}; // thank you.

            let found_surface: AtomicBool = false.into();
            with_surface_tree_downward(
                &window_surface,
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
            return PopupManager::popups_for_surface(&window_surface)
                .any(|(popup, _)| popup.wl_surface() == surface);
        }

        false
    }
}

// Geometry related functions
impl Tile {
    pub fn set_geometry(&mut self, mut new_geo: Rectangle<i32, Logical>, animate: bool) {
        if let Some(thickness) = self.border_thickness() {
            new_geo.loc += (thickness, thickness).into();
            new_geo.size -= (2 * thickness, 2 * thickness).into();
        }

        self.window.request_size(new_geo.size);
        self.rounded_corner_damage.set_size(new_geo.size);
        // TODO
        // if let Some(egui) = self.debug_overlay.as_mut() {
        //     egui.set_size(new_geo.size);
        // }

        self.set_location(new_geo.loc, animate);
    }

    pub fn set_location(&mut self, new_location: Point<i32, Logical>, animate: bool) {
        let mut old_location = self.location;
        if let Some(previous_animation) = self.location_animation.take() {
            old_location += previous_animation.value();
        }
        self.location = new_location;
        if animate {
            self.location_animation = LocationAnimation::new(
                old_location,
                new_location,
                self.config.animations.window_geometry.duration,
                self.config.animations.window_geometry.curve,
            );
        }
    }

    pub fn window_geometry(&self) -> Rectangle<i32, Logical> {
        Rectangle::from_loc_and_size(self.location, self.window.size())
    }

    pub fn window_visual_geometry(&self) -> Rectangle<i32, Logical> {
        Rectangle::from_loc_and_size(self.render_location(), self.window.size())
    }

    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        let mut geo = self.window_geometry();
        geo.loc = self.location;
        if let Some(thickness) = self.border_thickness() {
            geo.loc -= (thickness, thickness).into();
            geo.size += (2 * thickness, 2 * thickness).into();
        }
        geo
    }

    pub fn location(&self) -> Point<i32, Logical> {
        self.location
    }

    pub fn window_bbox(&self) -> Rectangle<i32, Logical> {
        let mut bbox = self.window.bbox();
        bbox.loc = self.location;
        bbox
    }

    pub fn render_location(&self) -> Point<i32, Logical> {
        let mut render_location = self.location;
        if let Some(offset) = self.location_animation.as_ref().map(LocationAnimation::value) {
            render_location += offset;
        }

        render_location
    }

    pub fn stop_location_animation(&mut self) {
        let _ = self.location_animation.take();
    }

    pub fn start_opening_animation(&mut self) {
        self.opening_animation = Some(
            Animation::new(0.0, 1.0, self.config.animations.window_open_close.duration)
                .with_curve(self.config.animations.window_open_close.curve),
        );
    }
}

// Animation-related code
impl Tile {
    pub fn prepare_close_animation(
        &mut self,
        renderer: &mut GlowRenderer,
        scale: Scale<f64>,
    ) -> bool {
        if self.close_animation_snapshot.is_some() {
            // We already prepared a close animation snapshot
            return true;
        }

        // NOTE: We use the border thickness as the location to actually include
        // it with the render elements, otherwise it
        // would be clipped out of the tile.
        let thickness = self.border_thickness().unwrap_or(0);
        let border_offset = Point::<i32, Logical>::from((thickness, thickness))
            .to_physical_precise_round::<_, i32>(scale);
        let elements = self
            .render_elements_inner(renderer, border_offset, scale, false)
            .collect::<Vec<_>>();
        self.close_animation_snapshot = Some(elements);
        true
    }

    pub fn clear_close_window_animation_snapshot(&mut self) {
        let _ = self.close_animation_snapshot.take();
    }

    pub fn advance_animations(&mut self, now: Duration) -> bool {
        let mut ret = false;

        let _ = self.location_animation.take_if(|anim| anim.is_finished());
        if let Some(location_animation) = self.location_animation.as_mut() {
            location_animation.tick(now);
            ret |= true;
        }

        let _ = self.opening_animation.take_if(|anim| anim.is_finished());
        if let Some(opening_animation) = self.opening_animation.as_mut() {
            opening_animation.tick(now);
            ret |= true;
        }

        ret
    }
}

impl Tile {
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
                        // TODO
                        // ui.indent("Window info", |ui| {
                        //     info(ui, "title", self.window.title());
                        //     info(ui, "app-id", self.window.app_id());
                        // });

                        ui.add_space(4.0);

                        ui.label("Window geometry");
                        ui.indent("Window geometry", |ui| {
                            info(ui, "location", {
                                let location = self.location;
                                format!("({}, {})", location.x, location.y).as_str()
                            });
                            info(ui, "size", {
                                let size = self.window.size();
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
                                self.window.fullscreen().to_string().as_str(),
                            );
                            info(
                                ui,
                                "maximized",
                                self.window.maximized().to_string().as_str(),
                            );
                            info(
                                ui,
                                "bounds",
                                self.window
                                    .bounds()
                                    .map(|bounds| format!("({}, {})", bounds.w, bounds.h))
                                    .unwrap_or_else(|| String::from("None"))
                                    .as_str(),
                            )
                        });
                    });
            });
    }

    fn render_elements_inner<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        focused: bool,
    ) -> impl Iterator<Item = TileRenderElement<R>> {
        let element_physical_geo = Rectangle::from_loc_and_size(
            location,
            self.window.size().to_physical_precise_round(scale),
        );
        let element_geo = element_physical_geo
            .to_f64()
            .to_logical(scale)
            .to_i32_round();

        let rules = self.window.rules();
        let alpha = if self.window.fullscreen() {
            1.0
        } else {
            rules.opacity.unwrap_or(1.0)
        };
        let border_config = self
            .config
            .decorations
            .border
            .with_overrides(&rules.border_overrides);
        // Mutex lock still exists until the iterator is collected.
        //
        // We use the WindowData associated with the winow (which holds the rules) else where, and
        // dropping this mutex guard is required to avoid a deadlock
        drop(rules);
        let need_rounding = !self.window.fullscreen();
        let radius = border_config.radius - border_config.thickness / 2.0;
        let border_thickness = if self.window.fullscreen() { None } else {
            // TODO: Get rid of truncating when switching to fractional layout
            Some(border_config.thickness as i32)
        };

        let window_elements = self
            .window
            .render_toplevel_elements(renderer, element_physical_geo.loc, scale, alpha)
            .into_iter()
            .map(move |e| {
                if !need_rounding {
                    return TileRenderElement::Element(e);
                }

                // Rounding off windows is a little tricky.
                //
                // Not every surface of the window means its "the window", not at all.
                // Some clients (like OBS-studio) use subsurfaces (not popups) to display different
                // parts of their interface (for example OBs does this with the preview window)
                //
                // To counter this, we check here if the surface is going to clip.
                if RoundedCornerElement::will_clip(&e, scale, element_geo, radius) {
                    let rounded = RoundedCornerElement::new(e, radius, element_geo, scale);
                    TileRenderElement::RoundedElement(rounded)
                } else {
                    TileRenderElement::Element(e)
                }
            });
        let popup_elements = self
            .window
            .render_popup_elements(renderer, element_physical_geo.loc, scale, alpha)
            .into_iter()
            .map(TileRenderElement::Element);

        // We need to have extra damage in the case we have a radius ontop of our window
        let damage = (radius != 0.0)
            .then(|| {
                let damage = self
                    .rounded_corner_damage
                    .clone()
                    .with_location(element_geo.loc);
                TileRenderElement::RoundedElementDamage(damage)
            })
            .into_iter();

        // Same deal for the border, only if the thickness is non-null
        let border_element = border_thickness
            .map(|thickness| {
                let mut border_geo = element_geo;
                border_geo.loc -= (thickness, thickness).into();
                border_geo.size += (2 * thickness, 2 * thickness).into();

                let border_element = RoundedOutlineElement::element(
                    renderer,
                    scale.x.max(scale.y),
                    alpha,
                    border_geo,
                    RoundedOutlineSettings {
                        half_thickness: border_config.thickness / 2.0,
                        radius,
                        color: if focused {
                            border_config.focused_color
                        } else {
                            border_config.normal_color
                        },
                    },
                );

                TileRenderElement::Border(border_element)
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
        scale: Scale<f64>,
        focused: bool,
    ) -> impl Iterator<Item = TileRenderElement<R>> {
        let mut render_geo = self
            .window_visual_geometry()
            .to_physical_precise_round(scale);

        // TODO
        // let debug_overlay = self
        //     .debug_overlay
        //     .as_ref()
        //     .map(|egui| {
        //         // TODO: Maybe use smithay's clock? But it just does this under the hood soo.
        //         use smithay::reexports::rustix;
        //         let time = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
        //         let time = Duration::new(time.tv_sec as u64, time.tv_nsec as u32);
        //         let element = egui
        //             .render(
        //                 renderer.glow_renderer_mut(),
        //                 scale.x as i32,
        //                 alpha,
        //                 render_geo.loc,
        //                 |ctx| self.egui_overlay(ctx),
        //                 time,
        //             )
        //             .unwrap();
        //         TileRenderElement::DebugOverlay(element)
        //     })
        //     .into_iter();

        let mut opening_element = None;
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
        let thickness = self.border_thickness().unwrap_or(0);
        let border_offset = Point::<i32, Logical>::from((thickness, thickness))
            .to_physical_precise_round::<_, i32>(scale);

        if let Some(animation) = self.opening_animation.as_ref() {
            let progress = *animation.value();

            let glow_renderer = renderer.glow_renderer_mut();
            // NOTE: We use the border thickness as the location to actually include it with the
            // render elements, otherwise it would be clipped out of the tile.
            let elements = self
                .render_elements_inner(glow_renderer, border_offset, scale, focused)
                .collect::<Vec<_>>();
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
            normal_elements =
                Some(self.render_elements_inner(renderer, render_geo.loc, scale, focused))
        }

        opening_element
            .into_iter()
            .chain(normal_elements.into_iter().flatten())
    }
}

crate::fht_render_elements! {
    TileRenderElement<R> => {
        Element = WaylandSurfaceRenderElement<R>,
        RoundedElement = RoundedCornerElement<WaylandSurfaceRenderElement<R>>,
        RoundedElementDamage = ExtraDamage,
        Border = FhtPixelShaderElement,
        DebugOverlay = EguiRenderElement,
        Opening = RescaleRenderElement<FhtTextureElement>,
    }
}

fn opening_animation_progress_to_scale(progress: f64) -> f64 {
    const OPEN_SCALE_THRESHOLD: f64 = 0.5;
    progress * (1.0 - OPEN_SCALE_THRESHOLD) + OPEN_SCALE_THRESHOLD
}

struct LocationAnimation {
    // Location animation holds the delta between the previous and current location. It gets
    // added to Tile.location until the offsets gets to zero
    x: Animation<i32>,
    y: Animation<i32>,
}

impl LocationAnimation {
    fn new(
        prev: Point<i32, Logical>,
        next: Point<i32, Logical>,
        duration: Duration,
        curve: AnimationCurve,
    ) -> Option<Self> {
        if prev == next {
            return None;
        }

        let delta = prev - next;
        Some(Self {
            x: Animation::new(delta.x, 0, duration).with_curve(curve),
            y: Animation::new(delta.y, 0, duration).with_curve(curve),
        })
    }

    fn tick(&mut self, now: Duration) {
        self.x.tick(now);
        self.y.tick(now);
    }

    fn is_finished(&self) -> bool {
        self.x.is_finished() || self.y.is_finished()
    }

    fn value(&self) -> Point<i32, Logical> {
        (*self.x.value(), *self.y.value()).into()
    }
}
