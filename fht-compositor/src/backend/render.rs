use egui::RichText;
use serde::{Deserialize, Serialize};
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::utils::{
    CropRenderElement, Relocate, RelocateRenderElement,
};
use smithay::backend::renderer::element::{AsRenderElements, Element, Kind, RenderElement};
use smithay::backend::renderer::gles::{GlesError, GlesTexture};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::{Frame, ImportAll, ImportMem, Renderer, Texture};
use smithay::desktop::{layer_map_for_output, PopupManager};
use smithay::input::pointer::CursorImageStatus;
use smithay::output::Output;
use smithay::utils::{IsAlive, Physical, Point, Rectangle, Scale};
use smithay::wayland::shell::wlr_layer::Layer;

#[cfg(feature = "udev_backend")]
use super::udev::{UdevFrame, UdevRenderError, UdevRenderer};
use crate::shell::cursor::CursorRenderElement;
use crate::shell::window::FhtWindowRenderElement;
use crate::state::Fht;
use crate::utils::geometry::RectGlobalExt;
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
{
    Cursor(CursorRenderElement<R>),
    Egui(TextureRenderElement<GlesTexture>),
    Wayland(WaylandSurfaceRenderElement<R>),
    WorkspaceSet(CropRenderElement<RelocateRenderElement<FhtWindowRenderElement<R>>>),
}

impl<R> From<WaylandSurfaceRenderElement<R>> for FhtRenderElement<R>
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: Clone + 'static,

    CursorRenderElement<R>: RenderElement<R>,

    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
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
{
    fn id(&self) -> &smithay::backend::renderer::element::Id {
        match self {
            Self::Cursor(e) => e.id(),
            Self::Egui(e) => e.id(),
            Self::Wayland(e) => e.id(),
            Self::WorkspaceSet(e) => e.id(),
        }
    }

    fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
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
        commit: Option<smithay::backend::renderer::utils::CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
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
) -> Vec<FhtRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem + AsGlowRenderer,
    <R as Renderer>::TextureId: Texture + Clone + 'static,

    CursorRenderElement<R>: RenderElement<R>,

    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
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

    if let Some(egui) = state
        .focus_state
        .output
        .as_ref()
        .and_then(|o| egui_elements(renderer.glow_renderer_mut(), o, &state))
    {
        elements.push(FhtRenderElement::Egui(egui))
    }

    let output_scale = output.current_scale().fractional_scale();
    let output_geo = output
        .geometry()
        .as_logical()
        .to_physical_precise_round(output_scale);

    let active = state.wset_for(output).active();
    let has_fullscreen = active.fullscreen.is_some();

    let overlay_elements = layer_elements(renderer, output, Layer::Overlay);
    elements.extend(overlay_elements);

    let mut window_elements = if has_fullscreen {
        vec![]
    } else {
        // Only render top layer shells if we dont have fullscreen elements
        // FIXME: This isn't good, since the fullscreen window may be transparent
        layer_elements(renderer, output, Layer::Top)
    };

    let active_elements = active.render_elements(renderer, output_scale.into(), 1.0);
    window_elements.extend(active_elements.into_iter().filter_map(|e| {
        let relocate = RelocateRenderElement::from_element(e, Point::default(), Relocate::Relative);
        let crop = CropRenderElement::from_element(relocate, output_scale, output_geo)?;
        Some(FhtRenderElement::WorkspaceSet(crop))
    }));

    elements.extend(window_elements);

    let background = layer_elements(renderer, output, Layer::Bottom)
        .into_iter()
        .chain(layer_elements(renderer, output, Layer::Background));
    elements.extend(background);

    elements
}

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
{
    let mut cursor_guard = state.cursor_theme_manager.image_status.lock().unwrap();
    let mut elements = vec![];

    let mut reset = false;
    if let CursorImageStatus::Surface(ref surface) = *cursor_guard {
        reset = !surface.alive();
    }
    if reset {
        *cursor_guard = CursorImageStatus::default_named();
    }
    drop(cursor_guard); // since its used by render_cursor

    let output_scale: Scale<f64> = output.current_scale().fractional_scale().into();
    let cursor_element_pos = state.pointer.current_location();
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

const EGUI_BACKGROUND: egui::Color32 = egui::Color32::from_rgb(21, 17, 16);
const EGUI_FOREGROUND: egui::Color32 = egui::Color32::from_rgb(227, 226, 232);
const CONFIG_ERROR_ACCENT: egui::Color32 = egui::Color32::from_rgb(233, 91, 97);

#[profiling::function]
fn egui_elements(
    renderer: &mut GlowRenderer,
    output: &Output,
    state: &Fht,
) -> Option<TextureRenderElement<GlesTexture>> {
    // WARN: If egui gets nothing rendered inside that closure down below it will use i32::min as
    // dimensions. Dont ask me why.
    if state.last_config_error.is_none() {
        return None;
    }

    state
        .egui
        .render(
            |ctx| {
                if let Some(err) = state.last_config_error.as_ref() {
                    egui_config_error(ctx, err);
                }
            },
            renderer,
            output.geometry().as_logical(),
            output.current_scale().fractional_scale(),
            1.0,
        )
        .ok()
}

fn egui_config_error(context: &egui::Context, error: &anyhow::Error) {
    let area = egui::Area::new("config_error").anchor(egui::Align2::CENTER_TOP, (10.0, 10.0));
    area.show(context, |ui| {
        egui::Frame::none()
            .fill(EGUI_BACKGROUND)
            .inner_margin(10.0)
            .stroke(egui::Stroke {
                width: 2.0,
                color: CONFIG_ERROR_ACCENT,
            })
            .show(ui, |ui| {
                ui.heading(
                    RichText::new("fht-compositor failed to reload your config!")
                        .color(CONFIG_ERROR_ACCENT),
                );
                ui.label(RichText::new(error.root_cause().to_string()).color(EGUI_FOREGROUND));
            })
    });
}

#[derive(Default, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BackendAllocator {
    Gbm,
    #[default]
    Vulkan,
}
