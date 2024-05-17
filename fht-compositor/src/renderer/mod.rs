//! The renderer part of fht-compositor.
//!
//! The renderer is the part responsible for drawing elements. In the smithay pipeline, this is
//! creating the various [`RenderElement`]s that are getting them submitted to a buffer attached to
//! an output that the renderer can bind to.
//!
//! This module also has some helpers to create render elements.

pub mod egui;
pub mod extra_damage;
pub mod pixel_shader_element;
pub mod render_elements;
pub mod rounded_element;
pub mod rounded_outline_shader;
pub mod texture_element;

use glam::Mat3;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{AsRenderElements, Kind, RenderElement};
use smithay::backend::renderer::gles::{
    GlesError, GlesRenderbuffer, GlesTexture, Uniform, UniformValue,
};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
#[cfg(feature = "udev_backend")]
use smithay::backend::renderer::multigpu::MultiTexture;
use smithay::backend::renderer::{Bind, Frame, ImportAll, ImportMem, Offscreen, Renderer, Texture};
use smithay::desktop::space::SurfaceTree;
use smithay::desktop::{layer_map_for_output, PopupManager};
use smithay::input::pointer::CursorImageStatus;
use smithay::output::Output;
use smithay::utils::{IsAlive, Scale};
use smithay::wayland::shell::wlr_layer::Layer;

use self::texture_element::FhtTextureElement;
#[cfg(feature = "udev_backend")]
use crate::backend::udev::UdevRenderError;
#[cfg(feature = "udev_backend")]
use crate::backend::udev::{UdevFrame, UdevRenderer};
use crate::config::CONFIG;
use crate::portals::CursorMode;
use crate::shell::cursor::CursorRenderElement;
use crate::shell::workspaces::WorkspaceSetRenderElement;
use crate::state::{egui_state_for_output, Fht, OutputState};
use crate::utils::fps::Fps;
use crate::utils::geometry::RectGlobalExt;
use crate::utils::output::OutputExt;

crate::fht_render_elements! {
    FhtRenderElement<R> => {
        Cursor = CursorRenderElement<R>,
        Color = SolidColorRenderElement,
        Egui = FhtTextureElement,
        Wayland = WaylandSurfaceRenderElement<R>,
        WorkspaceSet = WorkspaceSetRenderElement<R>,
    }
}

pub struct SplitRenderElements<E> {
    pub normal: Vec<E>,
    pub popups: Vec<E>,
}

pub struct OutputElementsResult<R: FhtRenderer> {
    pub render_elements: Vec<FhtRenderElement<R>>,
    pub cursor_elements_len: usize,
}

impl Fht {
    /// Generate all the elements for this output.
    pub fn output_elements<R: FhtRenderer>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        fps: &mut Fps,
    ) -> OutputElementsResult<R> {
        assert!(
            self.workspaces.get(output).is_some(),
            "Tried to render a non-existing output!"
        );

        let mut elements = vec![];

        // Start with the cursor
        //
        // Why include cursor_elements_len? Since if we are rendering to screencast, we can take a
        // slice of elements to skip cursor_elements (slice [cursor_elements_len..])
        let cursor_elements = self.cursor_elements(renderer, output);
        let cursor_elements_len = cursor_elements.len();
        elements.extend(cursor_elements);

        // Then EGUI, for debug overlay, config error notification, and greeting.
        if let Some(egui) = self.egui_elements(renderer.glow_renderer_mut(), output, fps) {
            elements.push(FhtRenderElement::Egui(FhtTextureElement(egui)))
        }

        // Then overlay layer shells + their popups
        let output_scale = output.current_scale().fractional_scale();
        let overlay_elements = layer_elements(renderer, output, Layer::Overlay);
        elements.extend(overlay_elements);

        // Then we come to Top layer shells and windows.
        // If we have a fullscreen window, it should be drawn above the Top layer shell, otherwise
        // draw the top layer then the rest of the windows.
        let (has_fullscreen, wset_elements) =
            self.wset_for(output)
                .render_elements(renderer, output_scale.into(), 1.0);
        if !has_fullscreen {
            elements.extend(layer_elements(renderer, output, Layer::Top));
            elements.extend(
                wset_elements
                    .into_iter()
                    .map(FhtRenderElement::WorkspaceSet),
            );
        } else {
            elements.extend(
                wset_elements
                    .into_iter()
                    .map(FhtRenderElement::WorkspaceSet),
            );
            elements.extend(layer_elements(renderer, output, Layer::Top));
        }

        // Finally we have background and bottom elements.
        let background = layer_elements(renderer, output, Layer::Bottom)
            .into_iter()
            .chain(layer_elements(renderer, output, Layer::Background));
        elements.extend(background);

        OutputElementsResult {
            render_elements: elements,
            cursor_elements_len,
        }
    }

    pub fn cursor_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        output: &Output,
    ) -> Vec<FhtRenderElement<R>> {
        let mut cursor_guard = self.cursor_theme_manager.image_status.lock().unwrap();

        if self
            .focus_state
            .output
            .as_ref()
            .is_some_and(|o| o != output)
        {
            // Do not render the cursor for a non-focused output.
            return vec![];
        }

        let mut reset = false;
        if let CursorImageStatus::Surface(ref surface) = *cursor_guard {
            reset = !surface.alive();
        }
        if reset {
            *cursor_guard = CursorImageStatus::default_named();
        }
        drop(cursor_guard); // since its used by render_cursor

        let output_scale = output.current_scale().fractional_scale().into();
        let cursor_element_pos =
            self.pointer.current_location() - output.current_location().to_f64();
        let cursor_element_pos_scaled = cursor_element_pos.to_physical(output_scale).to_i32_round();

        let cursor_scale = output.current_scale().integer_scale();
        let mut elements = self.cursor_theme_manager.render_cursor(
            renderer,
            cursor_element_pos_scaled,
            output_scale,
            cursor_scale,
            1.0,
            self.clock.now().into(),
        );

        // Draw drag and drop icon.
        if let Some(surface) = self.dnd_icon.as_ref().filter(IsAlive::alive) {
            elements.extend(AsRenderElements::<R>::render_elements(
                &SurfaceTree::from_surface(surface),
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
    /// integration of egui for smithay), calling the render function also runs the underlying
    /// context and sends events to it.
    ///
    /// This doesn't render anything if it figures that it's useless, but still dispatchs events to
    /// egui.
    #[profiling::function]
    fn egui_elements(
        &self,
        renderer: &mut GlowRenderer,
        output: &Output,
        fps: &mut Fps,
    ) -> Option<TextureRenderElement<GlesTexture>> {
        let scale = output.current_scale().fractional_scale();
        let egui = egui_state_for_output(output);
        if !CONFIG.renderer.debug_overlay && !CONFIG.greet && self.last_config_error.is_none() {
            // Even if we are rendering nothing, make sure egui understands we are really doing
            // nothing, because not running the context will make it use the last frame it was
            // drawn.
            //
            // It's also so we dispatch input events that we collected during the last frame
            egui.run(|_| (), scale);
            return None;
        } else {
            let is_focused = self
                .focus_state
                .output
                .as_ref()
                .is_some_and(|o| o == output);

            egui.render(
                |ctx| {
                    if CONFIG.renderer.debug_overlay {
                        egui::egui_debug_overlay(ctx, output, self, fps);
                    }

                    if is_focused && CONFIG.greet {
                        egui::egui_greeting_message(ctx);
                    }

                    if is_focused {
                        if let Some(err) = self.last_config_error.as_ref() {
                            egui::egui_config_error(ctx, err);
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

    /// Render and submit screencopy buffers using given renderer.
    #[cfg(feature = "xdg-screencast-portal")]
    #[profiling::function]
    pub fn render_screencast<R: FhtRenderer>(
        &mut self,
        output: &Output,
        renderer: &mut R,
        output_elements_result: &OutputElementsResult<R>,
    ) where
        FhtRenderElement<R>: RenderElement<R>,
    {
        let size = output.current_mode().unwrap().size;
        let transform = output.current_transform();
        let size = transform.transform_size(size);

        let scale = smithay::utils::Scale::from(output.current_scale().fractional_scale());

        let Some(pipewire) = self.pipewire.as_mut() else {
            return;
        };

        if pipewire.casts.is_empty() {
            return;
        }

        let dt = &mut OutputState::get(output).damage_tracker;
        let mut casts = std::mem::take(&mut pipewire.casts);
        let mut casts_to_stop = vec![];

        for cast in &mut casts {
            if !cast.is_active.get() {
                continue;
            }

            if &cast.output != output {
                continue;
            }

            if cast.size.to_physical_precise_round(scale) != size {
                casts_to_stop.push(cast.session_handle.clone());
                continue;
            }

            {
                let mut buffer = match cast.stream.dequeue_buffer() {
                    Some(buffer) => buffer,
                    None => {
                        debug!(
                            session_handle = cast.session_handle.to_string(),
                            "PipeWire stream out of buffers! Skipping frame."
                        );
                        continue;
                    }
                };

                let data = &mut buffer.datas_mut()[0];
                let fd = data.as_raw().fd as i32;
                let dmabuf = cast.dmabufs.borrow()[&fd].clone();

                let elements = if cast.cursor_mode.contains(CursorMode::EMBEDDED) {
                    &output_elements_result.render_elements
                } else {
                    &output_elements_result.render_elements
                        [output_elements_result.cursor_elements_len..]
                };

                if let Err(err) =
                    dt.render_output_with(renderer, dmabuf, 0, &elements, [0., 0., 0., 0.])
                {
                    error!(?err, "Failed to render elements to DMABUF");
                    continue;
                }

                let maxsize = data.as_raw().maxsize;
                let chunk = data.chunk_mut();
                *chunk.size_mut() = maxsize;
                *chunk.stride_mut() = maxsize as i32 / size.h;
            }
        }
        pipewire.casts = casts;

        for id in casts_to_stop {
            self.stop_cast(id);
        }
    }
}

/// A meta trait combining all the requirements for our renderer
pub trait FhtRenderer:
    Renderer<TextureId = Self::FhtTextureId, Error = Self::FhtError>
    + ImportAll
    + ImportMem
    + Bind<Dmabuf>
    + Offscreen<GlesRenderbuffer>
    + Offscreen<GlesTexture>
    + AsGlowRenderer
{
    // Thank you rust for not being able  to resolve type bounds.
    type FhtTextureId: Texture + Clone + 'static;
    type FhtError: std::error::Error + From<GlesError> + 'static;
}

impl FhtRenderer for GlowRenderer {
    type FhtTextureId = GlesTexture;
    type FhtError = GlesError;
}

#[cfg(feature = "udev_backend")]
impl<'a> FhtRenderer for UdevRenderer<'a> {
    type FhtTextureId = MultiTexture;
    type FhtError = UdevRenderError<'a>;
}

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

/// Initialize shaders for this renderer.
pub fn init_shaders(renderer: &mut impl AsGlowRenderer) {
    rounded_element::RoundedElementShader::init(renderer);
    rounded_outline_shader::RoundedOutlineShader::init(renderer);
}

// / Generate the layer shell elements for a given layer for a given output layer map.
pub fn layer_elements<R: FhtRenderer>(
    renderer: &mut R,
    output: &Output,
    layer: Layer,
) -> Vec<FhtRenderElement<R>> {
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

pub fn mat3_uniform(name: &str, mat: Mat3) -> Uniform {
    Uniform::new(
        name,
        UniformValue::Matrix3x3 {
            matrices: vec![mat.to_cols_array()],
            transpose: false,
        },
    )
}
