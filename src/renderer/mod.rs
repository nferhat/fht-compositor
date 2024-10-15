//! The renderer part of fht-compositor.
//!
//! The renderer is the part responsible for drawing elements. In the smithay pipeline, this is
//! creating the various [`RenderElement`]s that are getting them submitted to a buffer attached to
//! an output that the renderer can bind to.
//!
//! This module also has some helpers to create render elements.

pub mod extra_damage;
pub mod pixel_shader_element;
pub mod render_elements;
pub mod rounded_element;
pub mod shaders;
pub mod texture_element;

use anyhow::Context;
use glam::Mat3;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{AsRenderElements, RenderElement};
use smithay::backend::renderer::gles::{
    GlesError, GlesRenderbuffer, GlesTexture, Uniform, UniformValue,
};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
#[cfg(feature = "udev_backend")]
use smithay::backend::renderer::multigpu::MultiTexture;
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::{
    Bind, Color32F, Frame, ImportAll, ImportMem, Offscreen, Renderer, Texture,
};
use smithay::desktop::layer_map_for_output;
use smithay::desktop::space::SurfaceTree;
use smithay::input::pointer::CursorImageStatus;
use smithay::output::Output;
use smithay::utils::{IsAlive, Physical, Rectangle, Scale, Size, Transform};
use smithay::wayland::shell::wlr_layer::Layer;

#[cfg(feature = "udev_backend")]
use crate::backend::udev::UdevRenderError;
#[cfg(feature = "udev_backend")]
use crate::backend::udev::{UdevFrame, UdevRenderer};
use crate::shell::cursor::CursorRenderElement;
use crate::space::{MonitorRenderElement, MonitorRenderResult};
use crate::state::Fht;
use crate::utils::fps::Fps;

crate::fht_render_elements! {
    FhtRenderElement<R> => {
        Cursor = CursorRenderElement<R>,
        Debug = DebugRenderElement,
        Wayland = WaylandSurfaceRenderElement<R>,
        Monitor = MonitorRenderElement<R>,
    }
}

crate::fht_render_elements! {
    DebugRenderElement => {
        Damage = SolidColorRenderElement,
    }
}

/// Result of [`Fht::output_elements`].
///
/// Tracking of `cursor_elements_len` is done for screencast purposes (see
/// [`Fht::render_screencast`])
pub struct OutputElementsResult<R: FhtRenderer> {
    /// The render elements
    pub elements: Vec<FhtRenderElement<R>>,
    /// The render elements len.
    ///
    /// With how smithay does rendering, the cursor elements are always at the front of
    /// `self.elements`, to exclude the cursor elements from them, you can slice the vector at
    /// `[cursor_elements_len..]`
    pub cursor_elements_len: usize,
}

impl<R: FhtRenderer> Default for OutputElementsResult<R> {
    fn default() -> Self {
        Self {
            elements: vec![],
            cursor_elements_len: 0,
        }
    }
}

impl Fht {
    pub fn output_elements<R: FhtRenderer>(
        &mut self,
        renderer: &mut R,
        output: &Output,
        _fps: &mut Fps,
    ) -> OutputElementsResult<R> {
        let active_output = self.space.active_output();
        let monitor = self.space.active_monitor();
        // TODO: Fractional scale support.
        let output_scale = output.current_scale();
        let scale = (output_scale.integer_scale() as f64);

        let mut rv = OutputElementsResult::default();

        // Start with the cursor
        //
        // Why include cursor_elements_len? Since if we are rendering to screencast, we can take a
        // slice of elements to skip cursor_elements (slice [cursor_elements_len..])
        if active_output == output {
            // Render the cursor only on the active output
            let reset = matches!(
                self.cursor_theme_manager.image_status(),
                CursorImageStatus::Surface(ref surface) if !surface.alive()
            );
            if reset {
                self.cursor_theme_manager
                    .set_image_status(CursorImageStatus::default_named());
            }

            let cursor_element_pos = (self.pointer.current_location()
                - output.current_location().to_f64())
            .to_physical_precise_round(scale);
            if let Ok(elements) = self.cursor_theme_manager.render(
                renderer,
                cursor_element_pos,
                scale,
                output_scale.integer_scale(), // TODO: Fractional scale support
                1.0,
                self.clock.now().into(),
            ) {
                rv.cursor_elements_len += elements.len();
                rv.elements.extend(elements.into_iter().map(Into::into));
            }

            // Draw drag and drop icon.
            if let Some(surface) = self.dnd_icon.as_ref().filter(IsAlive::alive) {
                let elements = AsRenderElements::<R>::render_elements(
                    &SurfaceTree::from_surface(surface),
                    renderer,
                    cursor_element_pos,
                    scale.into(),
                    1.0,
                );
                rv.cursor_elements_len += elements.len();
                rv.elements.extend(elements);
            }
        }

        // Overlay layer shells are drawn above everything else, including fullscreen windows
        let overlay_elements = layer_elements(renderer, output, Layer::Overlay);
        rv.elements.extend(overlay_elements);

        // Top layer shells sit between the normal windows and fullscreen windows.
        let MonitorRenderResult {
            elements: monitor_elements,
            has_fullscreen,
        } = monitor.render(renderer, scale);
        if !has_fullscreen {
            rv.elements
                .extend(layer_elements(renderer, output, Layer::Top));
            rv.elements
                .extend(monitor_elements.into_iter().map(Into::into));
        } else {
            rv.elements
                .extend(monitor_elements.into_iter().map(Into::into));
            rv.elements
                .extend(layer_elements(renderer, output, Layer::Top));
        }

        // Finally we have background and bottom elements.
        let background = layer_elements(renderer, output, Layer::Bottom)
            .into_iter()
            .chain(layer_elements(renderer, output, Layer::Background));
        rv.elements.extend(background);

        rv
    }

    #[cfg(feature = "xdg-screencast-portal")]
    #[profiling::function]
    pub fn render_screencast<R: FhtRenderer>(
        &mut self,
        output: &Output,
        renderer: &mut R,
        output_elements_result: &OutputElementsResult<R>,
    ) where
        FhtRenderElement<R>: smithay::backend::renderer::element::RenderElement<R>,
    {
        use smithay::backend::renderer::Color32F;

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

        let dt = &mut crate::state::OutputState::get(output).damage_tracker;
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

                let elements = if cast
                    .cursor_mode
                    .contains(crate::portals::CursorMode::EMBEDDED)
                {
                    &output_elements_result.elements
                } else {
                    &output_elements_result.elements[output_elements_result.cursor_elements_len..]
                };

                if let Err(err) =
                    dt.render_output_with(renderer, dmabuf, 0, &elements, Color32F::TRANSPARENT)
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
    type FhtTextureId: Send + Texture + Clone + 'static;
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

pub fn layer_elements<R: FhtRenderer>(
    renderer: &mut R,
    output: &Output,
    layer: Layer,
) -> Vec<FhtRenderElement<R>> {
    let output_scale: Scale<f64> = output.current_scale().fractional_scale().into();
    let layer_map = layer_map_for_output(output);
    let output_loc = output.current_location();

    layer_map
        .layers_on(layer)
        .rev()
        .filter_map(|l| layer_map.layer_geometry(l).map(|geo| (geo.loc, l)))
        .flat_map(|(loc, layer)| {
            let location = (loc + output_loc).to_physical_precise_round(output_scale);
            layer.render_elements::<FhtRenderElement<R>>(renderer, location, output_scale, 1.0)
        })
        .collect()
}

#[profiling::function]
pub fn render_to_texture(
    renderer: &mut GlowRenderer,
    size: Size<i32, Physical>,
    scale: impl Into<Scale<f64>>,
    transform: Transform,
    fourcc: Fourcc,
    elements: impl Iterator<Item = impl RenderElement<GlowRenderer>>,
) -> anyhow::Result<(GlesTexture, SyncPoint)> {
    let scale = scale.into();
    let buffer_size = size.to_logical(1).to_buffer(1, Transform::Normal);

    let texture: GlesTexture = renderer
        .create_buffer(fourcc, buffer_size)
        .context("error creating texture")?;

    renderer
        .bind(texture.clone())
        .context("error binding texture")?;

    let transform = transform.invert();
    let output_rect = Rectangle::from_loc_and_size((0, 0), transform.transform_size(size));

    let mut frame = renderer
        .render(size, transform)
        .context("error starting frame")?;

    frame
        .clear(Color32F::TRANSPARENT, &[output_rect])
        .context("error clearing")?;

    for element in elements {
        let src = element.src();
        let dst = element.geometry(scale);

        if let Some(mut damage) = output_rect.intersection(dst) {
            damage.loc -= dst.loc;
            element
                .draw(&mut frame, src, dst, &[damage], &[])
                .context("error drawing element")?;
        }
    }

    let sync_point = frame.finish().context("error finishing frame")?;
    Ok((texture, sync_point))
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
