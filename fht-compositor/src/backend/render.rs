use serde::{Deserialize, Serialize};
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{AsRenderElements, Element, Kind, RenderElement};
use smithay::backend::renderer::gles::{GlesError, GlesTexture};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet};
use smithay::backend::renderer::{Frame, ImportAll, ImportMem, Renderer, Texture};
use smithay::desktop::{layer_map_for_output, PopupManager};
use smithay::input::pointer::CursorImageStatus;
use smithay::output::Output;
use smithay::utils::{IsAlive, Physical, Point, Rectangle, Scale};
use smithay::wayland::shell::wlr_layer::Layer;
use smithay_egui::{egui, egui_extras};

#[cfg(feature = "udev_backend")]
use super::udev::{UdevFrame, UdevRenderError, UdevRenderer};
use crate::config::CONFIG;
use crate::shell::cursor::CursorRenderElement;
use crate::shell::window::FhtWindowRenderElement;
use crate::shell::workspaces::{WorkspaceSetRenderElement, WorkspaceSwitchAnimation};
use crate::state::{egui_state_for_output, Fht};
use crate::utils::fps::Fps;
use crate::utils::geometry::{PointExt, PointGlobalExt, RectGlobalExt};
use crate::utils::output::OutputExt;

/// Helper trait to get around a borrow checker/trait checker limitations (e0277.
pub trait AsGlowRenderer: Renderer {
    fn glow_renderer(&self) -> &GlowRenderer;
    fn glow_renderer_mut(&mut self) -> &mut GlowRenderer;
}

pub trait AsGlowFrame<'frame>: Frame {
    fn glow_frame(&self) -> &GlowFrame<'frame>;
    fn glow_frame_mut(&mut self) -> &mut GlowFrame<'frame>;
}

impl AsGlowRenderer for GlowRenderer {
    fn glow_renderer(&self) -> &GlowRenderer {
        self
    }

    fn glow_renderer_mut(&mut self) -> &mut GlowRenderer {
        self
    }
}

impl<'frame> AsGlowFrame<'frame> for GlowFrame<'frame> {
    fn glow_frame(&self) -> &GlowFrame<'frame> {
        self
    }

    fn glow_frame_mut(&mut self) -> &mut GlowFrame<'frame> {
        self
    }
}

#[cfg(feature = "udev_backend")]
impl<'a> AsGlowRenderer for UdevRenderer<'a> {
    fn glow_renderer(&self) -> &GlowRenderer {
        self.as_ref()
    }

    fn glow_renderer_mut(&mut self) -> &mut GlowRenderer {
        self.as_mut()
    }
}

#[cfg(feature = "udev_backend")]
impl<'a, 'frame> AsGlowFrame<'frame> for UdevFrame<'a, 'frame> {
    fn glow_frame(&self) -> &GlowFrame<'frame> {
        self.as_ref()
    }

    fn glow_frame_mut(&mut self) -> &mut GlowFrame<'frame> {
        self.as_mut()
    }
}

#[derive(Debug)]
pub enum FhtRenderElement<R>
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: Clone + 'static,

    CursorRenderElement<R>: RenderElement<R>,
    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
    WorkspaceSetRenderElement<R>: RenderElement<R>,
{
    Cursor(CursorRenderElement<R>),
    Egui(TextureRenderElement<GlesTexture>),
    Wayland(WaylandSurfaceRenderElement<R>),
    WorkspaceSet(WorkspaceSetRenderElement<R>),
}

impl<R> From<WaylandSurfaceRenderElement<R>> for FhtRenderElement<R>
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: Clone + 'static,

    CursorRenderElement<R>: RenderElement<R>,

    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
    WorkspaceSetRenderElement<R>: RenderElement<R>,
{
    fn from(value: WaylandSurfaceRenderElement<R>) -> Self {
        Self::Wayland(value)
    }
}

impl<R> From<CursorRenderElement<R>> for FhtRenderElement<R>
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: Clone + 'static,

    CursorRenderElement<R>: RenderElement<R>,
    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
    WorkspaceSetRenderElement<R>: RenderElement<R>,
{
    fn from(value: CursorRenderElement<R>) -> Self {
        Self::Cursor(value)
    }
}

impl<R> Element for FhtRenderElement<R>
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: Texture + Clone + 'static,

    CursorRenderElement<R>: RenderElement<R>,
    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
    WorkspaceSetRenderElement<R>: RenderElement<R>,
{
    fn id(&self) -> &smithay::backend::renderer::element::Id {
        match self {
            Self::Cursor(e) => e.id(),
            Self::Egui(e) => e.id(),
            Self::Wayland(e) => e.id(),
            Self::WorkspaceSet(e) => e.id(),
        }
    }

    fn current_commit(&self) -> CommitCounter {
        match self {
            Self::Cursor(e) => e.current_commit(),
            Self::Egui(e) => e.current_commit(),
            Self::Wayland(e) => e.current_commit(),
            Self::WorkspaceSet(e) => e.current_commit(),
        }
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        match self {
            Self::Cursor(e) => e.location(scale),
            Self::Egui(e) => e.location(scale),
            Self::Wayland(e) => e.location(scale),
            Self::WorkspaceSet(e) => e.location(scale),
        }
    }

    fn src(&self) -> Rectangle<f64, smithay::utils::Buffer> {
        match self {
            Self::Cursor(e) => e.src(),
            Self::Egui(e) => e.src(),
            Self::Wayland(e) => e.src(),
            Self::WorkspaceSet(e) => e.src(),
        }
    }

    fn transform(&self) -> smithay::utils::Transform {
        match self {
            Self::Cursor(e) => e.transform(),
            Self::Egui(e) => e.transform(),
            Self::Wayland(e) => e.transform(),
            Self::WorkspaceSet(e) => e.transform(),
        }
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        match self {
            Self::Cursor(e) => e.geometry(scale),
            Self::Egui(e) => e.geometry(scale),
            Self::Wayland(e) => e.geometry(scale),
            Self::WorkspaceSet(e) => e.geometry(scale),
        }
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        match self {
            Self::Cursor(e) => e.damage_since(scale, commit),
            Self::Egui(e) => e.damage_since(scale, commit),
            Self::Wayland(e) => e.damage_since(scale, commit),
            Self::WorkspaceSet(e) => e.damage_since(scale, commit),
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        match self {
            Self::Cursor(e) => e.opaque_regions(scale),
            Self::Egui(e) => e.opaque_regions(scale),
            Self::Wayland(e) => e.opaque_regions(scale),
            Self::WorkspaceSet(e) => e.opaque_regions(scale),
        }
    }

    fn alpha(&self) -> f32 {
        match self {
            Self::Cursor(e) => e.alpha(),
            Self::Egui(e) => e.alpha(),
            Self::Wayland(e) => e.alpha(),
            Self::WorkspaceSet(e) => e.alpha(),
        }
    }

    fn kind(&self) -> Kind {
        match self {
            Self::Cursor(e) => e.kind(),
            Self::Egui(e) => e.kind(),
            Self::Wayland(e) => e.kind(),
            Self::WorkspaceSet(e) => e.kind(),
        }
    }
}

impl RenderElement<GlowRenderer> for FhtRenderElement<GlowRenderer> {
    fn draw(
        &self,
        frame: &mut GlowFrame,
        src: Rectangle<f64, smithay::utils::Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        match self {
            Self::Cursor(e) => e.draw(frame, src, dst, damage),
            Self::Egui(e) => {
                <TextureRenderElement<GlesTexture> as RenderElement<GlowRenderer>>::draw(
                    e, frame, src, dst, damage,
                )
            }
            Self::Wayland(e) => e.draw(frame, src, dst, damage),
            Self::WorkspaceSet(e) => e.draw(frame, src, dst, damage),
        }
    }

    fn underlying_storage(
        &self,
        renderer: &mut GlowRenderer,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage> {
        match self {
            Self::Cursor(e) => e.underlying_storage(renderer),
            Self::Egui(e) => e.underlying_storage(renderer),
            Self::Wayland(e) => e.underlying_storage(renderer),
            Self::WorkspaceSet(e) => e.underlying_storage(renderer),
        }
    }
}

#[cfg(feature = "udev_backend")]
impl<'a> RenderElement<UdevRenderer<'a>> for FhtRenderElement<UdevRenderer<'a>> {
    fn draw<'frame>(
        &self,
        frame: &mut UdevFrame<'a, 'frame>,
        src: Rectangle<f64, smithay::utils::Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), UdevRenderError> {
        match self {
            Self::Cursor(e) => e.draw(frame, src, dst, damage),
            Self::Egui(e) => {
                <TextureRenderElement<GlesTexture> as RenderElement<GlowRenderer>>::draw(
                    e,
                    frame.glow_frame_mut(),
                    src,
                    dst,
                    damage,
                )
                .map_err(|err| UdevRenderError::Render(err))
            }
            Self::Wayland(e) => e.draw(frame, src, dst, damage),
            Self::WorkspaceSet(e) => e.draw(frame, src, dst, damage),
        }
    }

    fn underlying_storage(
        &self,
        renderer: &mut UdevRenderer<'a>,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage> {
        match self {
            Self::Cursor(e) => e.underlying_storage(renderer),
            Self::Egui(e) => e.underlying_storage(renderer.glow_renderer_mut()),
            Self::Wayland(e) => e.underlying_storage(renderer),
            Self::WorkspaceSet(e) => e.underlying_storage(renderer),
        }
    }
}

#[profiling::function]
pub fn output_elements<R>(
    renderer: &mut R,
    output: &Output,
    state: &mut Fht,
    fps: &mut Fps,
) -> Vec<FhtRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem + AsGlowRenderer,
    <R as Renderer>::TextureId: Texture + Clone + 'static,

    CursorRenderElement<R>: RenderElement<R>,
    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
    WorkspaceSetRenderElement<R>: RenderElement<R>,
{
    let mut elements = cursor_elements(state, renderer, output);

    // How should we render? (more like the render order)
    //
    // 1. Egui info overlay/debug overlay (only on focused output)
    // 2. Overlay layer shells above everything, no questions asked.
    // 3. Fullscreen windows if any in the active workspace
    // 4. Top layer shells
    // 5. Normal non-fullscreen windows
    // 6. Bottom layer shells
    // 7. Background layer shells

    if let Some(egui) = egui_elements(renderer.glow_renderer_mut(), output, &state, fps) {
        elements.push(FhtRenderElement::Egui(egui))
    }

    let output_scale = output.current_scale().fractional_scale();

    let overlay_elements = layer_elements(renderer, output, Layer::Overlay);
    elements.extend(overlay_elements);

    let (has_fullscreen, wset_elements) =
        state
            .wset_for(output)
            .render_elements(renderer, output_scale.into(), 1.0);

    if !has_fullscreen {
        // Only render top layer shells if we dont have fullscreen elements
        // FIXME: This isn't good, since the fullscreen window may be transparent
        elements.extend(layer_elements(renderer, output, Layer::Top))
    };

    elements.extend(
        wset_elements
            .into_iter()
            .map(FhtRenderElement::WorkspaceSet),
    );

    let background = layer_elements(renderer, output, Layer::Bottom)
        .into_iter()
        .chain(layer_elements(renderer, output, Layer::Background));
    elements.extend(background);

    elements
}

/// Generate the layer shell elements for a given layer for a given output layer map.
#[profiling::function]
pub fn layer_elements<R>(
    renderer: &mut R,
    output: &Output,
    layer: Layer,
) -> Vec<FhtRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: Texture + Clone + 'static,

    CursorRenderElement<R>: RenderElement<R>,
    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
    WorkspaceSetRenderElement<R>: RenderElement<R>,
{
    let output_scale: Scale<f64> = output.current_scale().fractional_scale().into();

    let layer_map = layer_map_for_output(output);
    let mut elements = vec![];

    for (location, layer) in layer_map
        .layers_on(layer)
        .rev()
        .filter_map(|l| layer_map.layer_geometry(l).map(|geo| (geo.loc, l)))
    {
        let location = location.to_physical_precise_round(output_scale);
        let wl_surface = layer.wl_surface();

        elements.extend(PopupManager::popups_for_surface(wl_surface).flat_map(
            |(popup, offset)| {
                let offset = (offset - popup.geometry().loc)
                    .to_f64()
                    .to_physical_precise_round(output_scale);
                render_elements_from_surface_tree(
                    renderer,
                    popup.wl_surface(),
                    location + offset,
                    output_scale,
                    1.0,
                    Kind::Unspecified,
                )
            },
        ));

        elements.extend(render_elements_from_surface_tree(
            renderer,
            wl_surface,
            location,
            output_scale,
            1.0,
            Kind::Unspecified,
        ));
    }

    elements
}

/// Generate cursor elements for a given output.
#[profiling::function]
pub fn cursor_elements<R>(
    state: &Fht,
    renderer: &mut R,
    output: &Output,
) -> Vec<FhtRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem + AsGlowRenderer,
    <R as Renderer>::TextureId: Clone + 'static,
    CursorRenderElement<R>: RenderElement<R>,
    FhtWindowRenderElement<R>: RenderElement<R>,
    WorkspaceSetRenderElement<R>: RenderElement<R>,
{
    let mut cursor_guard = state.cursor_theme_manager.image_status.lock().unwrap();
    let mut elements = vec![];

    if state
        .focus_state
        .output
        .as_ref()
        .is_some_and(|o| o != output)
    {
        return elements;
    }

    let mut reset = false;
    if let CursorImageStatus::Surface(ref surface) = *cursor_guard {
        reset = !surface.alive();
    }
    if reset {
        *cursor_guard = CursorImageStatus::default_named();
    }
    drop(cursor_guard); // since its used by render_cursor

    let output_scale: Scale<f64> = output.current_scale().fractional_scale().into();
    let cursor_element_pos = state.pointer.current_location() - output.current_location().to_f64();
    let cursor_element_pos_scaled = cursor_element_pos.to_physical(output_scale).to_i32_round();

    let cursor_scale = output.current_scale().integer_scale();
    elements.extend(state.cursor_theme_manager.render_cursor(
        renderer,
        cursor_element_pos_scaled,
        output_scale,
        cursor_scale,
        1.0,
        state.clock.now().into(),
    ));

    // Draw drag and drop icon.
    if let Some(surface) = state.dnd_icon.as_ref().filter(IsAlive::alive) {
        elements.extend(AsRenderElements::<R>::render_elements(
            &smithay::desktop::space::SurfaceTree::from_surface(surface),
            renderer,
            cursor_element_pos_scaled,
            output_scale,
            1.0,
        ));
    }

    elements
}

/// Generate the egui elements for a given [`Output`]
///
/// However, this function does more than simply render egui, due to how smithay-egui works (the
/// integration of egui for smithay), calling the render function also runs the underlying context
/// and sends events to it.
///
/// This doesn't render anything if it figures that it's useless, but still dispatchs events to
/// egui.
#[profiling::function]
fn egui_elements(
    renderer: &mut GlowRenderer,
    output: &Output,
    state: &Fht,
    fps: &mut Fps,
) -> Option<TextureRenderElement<GlesTexture>> {
    let scale = output.current_scale().fractional_scale();
    let egui = egui_state_for_output(output);
    if !CONFIG.renderer.debug_overlay && !CONFIG.greet && state.last_config_error.is_none() {
        // Even if we are rendering nothing, make sure egui understands we are really doing
        // nothing, because not running the context will make it use the last frame it was rendered
        // with, so the one with the windows and whatnot
        //
        // It's also so we dispatch input events that we collected during the last frame
        egui.run(|_| (), scale);
        return None;
    } else {
        let is_focused = state
            .focus_state
            .output
            .as_ref()
            .is_some_and(|o| o == output);

        egui.render(
            |ctx| {
                if CONFIG.renderer.debug_overlay {
                    egui_debug_overlay(ctx, output, state, fps);
                }

                if is_focused && CONFIG.greet {
                    egui_greeting_message(ctx);
                }

                if is_focused {
                    if let Some(err) = state.last_config_error.as_ref() {
                        egui_config_error(ctx, err);
                    }
                }
            },
            renderer,
            output.geometry().as_logical().loc,
            scale,
            1.0,
        )
        .ok()
    }
}

/// Render the egui debug overlay for this output.
#[profiling::function]
fn egui_debug_overlay(context: &egui::Context, output: &Output, state: &Fht, fps: &mut Fps) {
    let area = egui::Window::new(output.name())
        .resizable(false)
        .collapsible(false)
        .movable(true);
    let mode = output.current_mode().unwrap();
    let scale = output.current_scale().fractional_scale();
    let pointer_loc = state
        .pointer
        .current_location()
        .as_global()
        .to_local(output);
    let geo = output.geometry();
    let wset = state.wset_for(output);

    let active_idx_str = if let Some(WorkspaceSwitchAnimation { ref target_idx, .. }) =
        wset.switch_animation.as_ref()
    {
        format!(
            "{active_idx} => {target_idx}",
            active_idx = wset.get_active_idx()
        )
    } else {
        wset.get_active_idx().to_string()
    };

    let (max_frametime, min_frametime, avg_frametime, avg_fps) = (
        fps.max_frametime().as_secs_f64() * 1_000.0,
        fps.min_frametime().as_secs_f64() * 1_000.0,
        fps.avg_frametime().as_secs_f64() * 1_000.0,
        fps.avg_fps(),
    );
    let avg_rendertime = fps.avg_rendertime(5).as_secs_f64();

    let format_info = |ui: &mut egui::Ui, name, data| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(format!("{name}: "));
            ui.code(data);
        });
    };

    area.show(context, |ui| {
        ui.collapsing("Framerate information", |ui| {
            format_info(ui, "FPS", format!("{:0>07.3}", avg_fps));
            format_info(
                ui,
                "Average rendertime",
                format!("{:0>07.3}", avg_rendertime),
            );
            format_info(ui, "Minimum frametime", format!("{:0>07.3}", min_frametime));
            format_info(ui, "Average frametime", format!("{:0>07.3}", avg_frametime));
            format_info(ui, "Maximum frametime", format!("{:0>07.3}", max_frametime));
        });

        ui.collapsing("Mode information", |ui| {
            format_info(ui, "Refresh rate", format!("{}", mode.refresh / 1_000));
            format_info(
                ui,
                "Size in pixels",
                format!("{}x{}", mode.size.w, mode.size.h),
            );
            format_info(
                ui,
                "Current location",
                format!("({}, {})", geo.loc.x, geo.loc.y),
            );
            format_info(ui, "Current scale", format!("{:0>04.2}", scale))
        });

        ui.collapsing("Misc information", |ui| {
            format_info(
                ui,
                "Pointer location",
                format!("({:0>09.4}, {:0>09.4})", pointer_loc.x, pointer_loc.y),
            );
            format_info(ui, "Active workspace idx", active_idx_str);
            format_info(ui, "Animations ongoing", format!("Figure this out"));
        });
    });
}

/// Render the config error notification for this output
#[profiling::function]
fn egui_config_error(context: &egui::Context, error: &anyhow::Error) {
    let area = egui::Window::new("Failed to reload config!")
        .anchor(egui::Align2::CENTER_TOP, (0.0, 10.0))
        .resizable(false)
        .collapsible(false)
        .movable(true);
    area.show(context, |ui| {
        ui.label(error.to_string());
        ui.label(error.root_cause().to_string());
    });
}

const USEFUL_DEFAULT_KEYBINDS: [(&str, &str); 8] = [
    ("Mod+Return", "Spawn alacritty"),
    ("Mod+P", "Launch `wofi --show drun`"),
    ("Mod+Q", "Exit the compositor"),
    ("Mod+Ctrl+R", "Reload the configuration"),
    ("Mod+J", "Focus the next window"),
    ("Mod+K", "Focus the previous window"),
    ("Mod+1-9", "Focus the nth workspace"),
    (
        "Mod+Shift+1-9",
        "Send the focused window to the nth workspace",
    ),
];

/// Render the greeting message for this output.
#[profiling::function]
fn egui_greeting_message(context: &egui::Context) {
    let area = egui::Window::new("Welcome to fht-compositor").resizable(false);
    area.show(context, |ui| {
        ui.label("If you are seeing this message, that means you successfully installed and ran the compositor with no issues! Congratulations!");

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("The compositor should have now copied a starter configuration to the following path:");
            ui.code("$XDG_CONFIG_HOME/.config/fht/compositor.ron");
        });

        ui.add_space(8.0);
        ui.label("You can disable this message by setting greet to false in your config file!");

        ui.add_space(12.0);
        ui.heading("Warning notice");
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label("Bear in mind that fht-compositor is STILL an alpha-quality software, and breaking changes can and will happen. ");
            ui.label("If you encounter any issues, or want to contribute, feel free to check out the ");
            ui.hyperlink_to("github page.", "https://github.com/nferhat/fht-shell/blob/main/fht-compositor/");
        });

        ui.add_space(12.0);
        ui.label("Some useful keybinds to know that are in this default config:");
        egui_extras::TableBuilder::new(ui)
            .column(egui_extras::Column::exact(100.0))
            .column(egui_extras::Column::remainder())
            .striped(true)
            .header(15.0, |mut header_row| {
                header_row.col(|ui| { ui.label("Key pattern"); });
                header_row.col(|ui| { ui.label("Description"); });
            })
            .body(|mut body| {
                for (key_pattern, description) in USEFUL_DEFAULT_KEYBINDS {
                    body.row(15.0, |mut row| {
                        row.col(|ui| { ui.code(key_pattern); });
                        row.col(|ui| { ui.label(description); });
                    });
                }
            });
    });
}

#[derive(Default, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BackendAllocator {
    Gbm,
    #[default]
    Vulkan,
}
