//! A single workspace tile.

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
use smithay::desktop::{PopupManager, WindowSurfaceType};
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
use crate::window::Window;

pub struct Tile {
    window: Window,
    location: Point<i32, Logical>,
    cfact: f32,
    border_config: Option<BorderConfig>,
    rounded_corner_damage: ExtraDamage,
    temporary_render_location: Option<Point<i32, Logical>>,
    location_animation: Option<Animation<Point<i32, Logical>>>,
    open_close_animation: Option<OpenCloseAnimation>,
    close_animation_snapshot: Option<Vec<WorkspaceTileRenderElement<GlowRenderer>>>,
    debug_overlay: Option<EguiElement>,
}

impl PartialEq for Tile {
    fn eq(&self, other: &Self) -> bool {
        self.window == other.window
    }
}

impl Tile {
    pub fn new(window: Window, border_config: Option<BorderConfig>) -> Self {
        let window_size = window.size();

        Self {
            window,
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
                .then(|| EguiElement::new(window_size)),
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

    pub fn into_window(self) -> (Window, Option<BorderConfig>) {
        (self.window, self.border_config)
    }

    pub fn send_pending_configure(&mut self) {
        self.window.send_pending_configure();
    }

    pub fn border_config(&self) -> BorderConfig {
        self.border_config.unwrap_or(CONFIG.decoration.border)
    }

    pub fn border_thickness(&self) -> Option<i32> {
        if self.window.fullscreen() {
            return None;
        }

        Some(self.border_config().thickness as i32)
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
        let mut render_location = self.temporary_render_location.unwrap_or(self.location);
        if let Some(offset) = self.location_animation.as_ref().map(Animation::value) {
            render_location += offset;
        }

        render_location
    }

    pub fn stop_location_animation(&mut self) {
        let _ = self.location_animation.take();
    }

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
}

// Animation-related code
impl Tile {
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

    pub fn is_closing(&self) -> bool {
        matches!(self.open_close_animation, Some(OpenCloseAnimation::Closing { .. }))
    }

    pub fn clear_close_snapshot(&mut self) {
        let _ = self.close_animation_snapshot.take();
    }

    pub fn start_close_animation(&mut self, renderer: &mut GlowRenderer, scale: Scale<f64>) {
        let Some(elements) = self.close_animation_snapshot.take() else {
            return;
        };
        let thickness = self.border_config().thickness as i32;
        let tile_size = self.window.size() + (thickness * 2, thickness * 2).into();

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

    fn render_elements_inner<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
        focused: bool,
    ) -> impl Iterator<Item = WorkspaceTileRenderElement<R>> {
        let element_physical_geo = Rectangle::from_loc_and_size(
            location,
            self.window.size().to_physical_precise_round(scale),
        );
        let element_geo = element_physical_geo
            .to_f64()
            .to_logical(scale)
            .to_i32_round();

        let border_config = self.border_config.unwrap_or(CONFIG.decoration.border);
        let need_rounding = !self.window.fullscreen();
        let radius = border_config.radius();

        let window_elements = self
            .window
            .render_toplevel_elements(renderer, element_physical_geo.loc, scale, alpha)
            .into_iter()
            .map(move |e| {
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
                if RoundedCornerElement::will_clip(&e, scale, element_geo, radius) {
                    let rounded = RoundedCornerElement::new(e, radius, element_geo, scale);
                    WorkspaceTileRenderElement::RoundedElement(rounded)
                } else {
                    WorkspaceTileRenderElement::Element(e)
                }
            });
        let popup_elements = self
            .window
            .render_popup_elements(renderer, element_physical_geo.loc, scale, alpha)
            .into_iter()
            .map(WorkspaceTileRenderElement::Element);

        // We need to have extra damage in the case we have a radius ontop of our window
        let damage = (radius != 0.0)
            .then(|| {
                let damage = self
                    .rounded_corner_damage
                    .clone()
                    .with_location(element_geo.loc);
                WorkspaceTileRenderElement::RoundedElementDamage(damage)
            })
            .into_iter();

        // Same deal for the border, only if the thickness is non-null
        let border_element = self
            .border_thickness()
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
        scale: Scale<f64>,
        alpha: f32,
        focused: bool,
    ) -> impl Iterator<Item = WorkspaceTileRenderElement<R>> {
        let mut render_geo = self
            .window_visual_geometry()
            .to_physical_precise_round(scale);

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

        let mut open_close_element = None;
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

            open_close_element = render_to_texture(
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
                let rescale = (progress * (1.0 - OpenCloseAnimation::OPEN_SCALE_THRESHOLD))
                    + OpenCloseAnimation::OPEN_SCALE_THRESHOLD;
                let rescale = RescaleRenderElement::from_element(texture, origin, rescale);

                WorkspaceTileRenderElement::<R>::OpenClose(
                    WorkspaceTileOpenCloseElement::OpenTexture(rescale),
                )
            });
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

            open_close_element = Some(element)
        };

        if open_close_element.is_none() {
            self.window.set_offscreen_element_id(None);
            normal_elements =
                Some(self.render_elements_inner(renderer, render_geo.loc, scale, alpha, focused))
        }

        debug_overlay
            .chain(open_close_element.into_iter())
            .chain(normal_elements.into_iter().flatten())
    }
}

impl IsAlive for Tile {
    fn alive(&self) -> bool {
        if matches!(
            &self.open_close_animation,
            Some(anim) if !anim.is_finished()
        ) {
            // We do not want to clear our the window if we opening/closing
            return true;
        }
        self.window.alive()
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
