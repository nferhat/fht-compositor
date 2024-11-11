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
use smithay::backend::allocator::{Buffer, Fourcc};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::{self, AsRenderElements, Kind, RenderElement};
use smithay::backend::renderer::gles::{
    GlesError, GlesMapping, GlesTexture, Uniform, UniformValue,
};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{
    Bind, Blit, Color32F, ExportMem, Frame, ImportAll, ImportMem, Offscreen, Renderer, Texture,
    TextureFilter, TextureMapping,
};
use smithay::desktop::layer_map_for_output;
use smithay::desktop::space::SurfaceTree;
use smithay::input::pointer::CursorImageStatus;
use smithay::output::{Output, OutputModeSource};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, Mode, PostAction};
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::utils::{IsAlive, Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::shell::wlr_layer::Layer;
use smithay::wayland::shm::with_buffer_contents_mut;

use crate::config::ui::ConfigUiRenderElement;
use crate::output::OutputExt;
use crate::protocols::screencopy::{ScreencopyBuffer, ScreencopyFrame};
use crate::shell::cursor::CursorRenderElement;
use crate::space::{MonitorRenderElement, MonitorRenderResult};
use crate::state::Fht;
use crate::utils::get_monotonic_time;

crate::fht_render_elements! {
    FhtRenderElement<R> => {
        Cursor = CursorRenderElement<R>,
        ConfigUi = ConfigUiRenderElement,
        Solid = SolidColorRenderElement,
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

const LOCKED_OUTPUT_BACKDROP_COLOR: Color32F = Color32F::new(0.0, 0.0, 0.0, 0.0);

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
    ) -> OutputElementsResult<R> {
        crate::profile_function!();
        let active_output = self.space.active_output();
        let monitor = self.space.active_monitor();
        // TODO: Fractional scale support.
        let output_scale = output.current_scale();
        let scale = output_scale.integer_scale() as f64;

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

        if !self.config_ui.hidden() {
            // Draw config ui below cursor, only if we didnt start drawing it on another output.
            let config_ui_output = self.config_ui_output.get_or_insert_with(|| output.clone());
            if config_ui_output == output {
                if let Some(element) = self.config_ui.render(renderer, output, scale) {
                    rv.elements.push(element.into())
                }
            }
        } else {
            let _ = self.config_ui_output.take();
        }

        // Render session lock surface between output and elements
        if self.is_locked() {
            let output_state = self.output_state.get_mut(output).unwrap();
            if let Some(lock_surface) = output_state.lock_surface.as_ref() {
                rv.elements.extend(render_elements_from_surface_tree(
                    renderer,
                    lock_surface.wl_surface(),
                    Point::default(),
                    scale,
                    1.0,
                    Kind::Unspecified,
                ));
            } else {
                // We still render a black drop to not show desktop content
                let mut output_geo = output.geometry().to_physical_precise_round(scale);
                output_geo.loc = Point::default(); // render at output origin.
                rv.elements.push(
                    SolidColorRenderElement::new(
                        element::Id::new(),
                        output_geo,
                        CommitCounter::default(),
                        LOCKED_OUTPUT_BACKDROP_COLOR,
                        Kind::Unspecified,
                    )
                    .into(),
                );
            }

            output_state.has_lock_backdrop = true;
        }

        // Overlay layer shells are drawn above everything else, including fullscreen windows
        let overlay_elements = layer_elements(renderer, output, Layer::Overlay);
        rv.elements.extend(overlay_elements);

        // Top layer shells sit between the normal windows and fullscreen windows.
        //
        // NOTE: About the location of render elements.
        // The compositor logic for now does not have a notion of "global space", similar to what
        // smithay::desktop::Space provides, but instead, each tile stores its location relative to
        // the output its mapped in.
        //
        // We do not have to offset the render elements in order to position them on the Output.
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
    pub fn render_screencast<R: FhtRenderer>(
        &mut self,
        output: &Output,
        renderer: &mut R,
        output_elements_result: &OutputElementsResult<R>,
    ) where
        FhtRenderElement<R>: element::RenderElement<R>,
    {
        crate::profile_function!();
        use crate::utils::pipewire::CastSource;

        let size = output.current_mode().unwrap().size;
        let transform = output.current_transform();
        let size = transform.transform_size(size);
        let scale = output.current_scale().fractional_scale().into();
        let source = CastSource::Output(output.downgrade());

        let Some(pipewire) = self.pipewire.as_mut() else {
            return;
        };

        if pipewire.casts.is_empty() {
            return;
        }

        let mut casts = std::mem::take(&mut pipewire.casts);
        let mut casts_to_stop = vec![];

        for cast in &mut casts {
            crate::profile_scope!("render_cast", cast.id().to_string());
            if !cast.active() {
                trace!(id = ?cast.id(), "Cast is not active, skipping");
                continue;
            }

            if *cast.source() != source {
                continue;
            }

            match cast.ensure_size(size) {
                Ok(true) => (),
                Ok(false) => {
                    trace!(id = ?cast.id(), "Cast is resizing, skipping");
                    continue;
                }
                Err(err) => {
                    warn!("error updating stream size, stopping screencast: {err:?}");
                    casts_to_stop.push(cast.id());
                }
            }

            if let Err(err) = cast.render(renderer, output_elements_result, size, scale) {
                error!(id = ?cast.id(), ?err, "Failed to render cast");
            }
        }
        pipewire.casts = casts;

        for id in casts_to_stop {
            self.stop_cast(id);
        }
    }

    pub fn render_screencopy_without_damage<R: FhtRenderer>(
        &mut self,
        output: &Output,
        renderer: &mut R,
        output_elements_result: &OutputElementsResult<R>,
    ) where
        FhtRenderElement<R>: RenderElement<R>,
    {
        crate::profile_function!();
        let output_state = self.output_state.get_mut(output).unwrap();
        let (with_damage, without_damage) = output_state
            .pending_screencopies
            .drain(..)
            .partition(|scrpy| scrpy.with_damage());
        output_state.pending_screencopies = with_damage;

        for screencopy in without_damage {
            match render_screencopy_internal(
                &screencopy,
                &mut output_state.screencopy_damage_tracker,
                renderer,
                output_elements_result,
            ) {
                Ok((sync_point, _)) => {
                    let submit_time = get_monotonic_time();
                    let Some(sync_point) = sync_point.and_then(|sp| sp.export()) else {
                        screencopy.submit(false, submit_time);
                        return;
                    };

                    let generic = Generic::new(sync_point, Interest::READ, Mode::OneShot);
                    let mut screencopy = Some(screencopy);
                    if let Err(err) = self.loop_handle.insert_source(generic, move |_, _, _| {
                        screencopy.take().unwrap().submit(false, submit_time);
                        Ok(PostAction::Remove)
                    }) {
                        error!("Failed to set screencopy sync point source: {err:?}");
                    }
                }
                Err(err) => {
                    error!("Failed to render for screencopy: {err:?}");
                    screencopy.failed();
                }
            }
        }
    }

    pub fn render_screencopy_with_damage<R: FhtRenderer>(
        &mut self,
        output: &Output,
        renderer: &mut R,
        output_elements_result: &OutputElementsResult<R>,
    ) where
        FhtRenderElement<R>: RenderElement<R>,
    {
        crate::profile_function!();
        let output_state = self.output_state.get_mut(output).unwrap();
        let (with_damage, without_damage) = output_state
            .pending_screencopies
            .drain(..)
            .partition(|scrpy| scrpy.with_damage());
        output_state.pending_screencopies = without_damage;

        for mut screencopy in with_damage {
            match render_screencopy_internal(
                &screencopy,
                &mut output_state.screencopy_damage_tracker,
                renderer,
                output_elements_result,
            ) {
                Ok((sync_point, damage)) => {
                    if let Some(damage) = damage {
                        // NOTE: Should we submit damage relative to output or relative to
                        // screencopy region? No clients seem to use the
                        // submitted (wf-recorder and friends set a noop)
                        screencopy.damage(damage);
                    }

                    let submit_time = get_monotonic_time();
                    let Some(sync_point) = sync_point.and_then(|sp| sp.export()) else {
                        screencopy.submit(false, submit_time);
                        return;
                    };

                    let generic = Generic::new(sync_point, Interest::READ, Mode::OneShot);
                    let mut screencopy = Some(screencopy);
                    if let Err(err) = self.loop_handle.insert_source(generic, move |_, _, _| {
                        screencopy.take().unwrap().submit(false, submit_time);
                        Ok(PostAction::Remove)
                    }) {
                        error!("Failed to set screencopy sync point source: {err:?}");
                    }
                }
                Err(err) => {
                    error!("Failed to render for screencopy: {err:?}");
                    screencopy.failed();
                }
            }
        }
    }
}

/// Trait to abstract away renderer requirements from function declarations.
pub trait FhtRenderer:
    Renderer<TextureId = Self::FhtTextureId, Error = Self::FhtError>
    + ImportAll
    + ImportMem
    + Bind<Dmabuf>
    + Blit<Dmabuf>
    // The renderers are just wrappers around a GlowRenderer.
    // So we can create GlesTexture and Bind to them with no issues.
    + Offscreen<GlesTexture>
    + Bind<GlesTexture>
    + ExportMem<TextureMapping = Self::FhtTextureMapping>
    + AsGlowRenderer
{
    // Thank you rust for not being able  to resolve type bounds.
    type FhtTextureId: Send + Texture + Clone + 'static;
    type FhtError: std::error::Error + Send + Sync + From<GlesError> + 'static;
    type FhtTextureMapping: Send + TextureMapping + 'static;
}

impl FhtRenderer for GlowRenderer {
    type FhtTextureId = GlesTexture;
    type FhtError = GlesError;
    type FhtTextureMapping = GlesMapping;
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

pub fn layer_elements<R: FhtRenderer>(
    renderer: &mut R,
    output: &Output,
    layer: Layer,
) -> Vec<FhtRenderElement<R>> {
    crate::profile_function!();
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

/// Render the given `elements` inside a [`GlesTexture`].
///
/// It is up to **YOU** to unbind the renderer after rendering.
pub fn render_to_texture<R: FhtRenderer>(
    renderer: &mut R,
    size: Size<i32, Physical>,
    scale: impl Into<Scale<f64>>,
    transform: Transform,
    fourcc: Fourcc,
    elements: impl Iterator<Item = impl RenderElement<R>>,
) -> anyhow::Result<(GlesTexture, SyncPoint)> {
    crate::profile_function!();
    let scale = scale.into();
    let buffer_size = size.to_logical(1).to_buffer(1, Transform::Normal);

    let texture = renderer
        .create_buffer(fourcc, buffer_size)
        .context("error creating texture")?;
    renderer
        .bind(texture.clone())
        .context("error binding texture")?;
    let sync_point = render_elements(renderer, size, scale, transform, elements)?;

    Ok((texture, sync_point))
}

/// Render the given `elements` inside the current bound target.
///
/// It is up to **YOU** to bind and unbind the renderer before and after calling this function.
pub fn render_elements<R: FhtRenderer>(
    renderer: &mut R,
    size: Size<i32, Physical>,
    scale: impl Into<Scale<f64>>,
    transform: Transform,
    elements: impl Iterator<Item = impl RenderElement<R>>,
) -> anyhow::Result<SyncPoint> {
    let scale = scale.into();
    let transform = transform.invert();
    let frame_rect = Rectangle::from_loc_and_size((0, 0), transform.transform_size(size));
    let mut frame = renderer
        .render(size, transform)
        .context("error starting frame")?;

    frame
        .clear(Color32F::TRANSPARENT, &[frame_rect])
        .context("error clearing")?;

    for element in elements {
        let src = element.src();
        let dst = element.geometry(scale);

        if let Some(mut damage) = frame_rect.intersection(dst) {
            damage.loc -= dst.loc;
            element
                .draw(&mut frame, src, dst, &[damage], &[])
                .context("error drawing element")?;
        }
    }

    frame.finish().context("error finishing frame")
}

fn render_screencopy_internal<'a, R: FhtRenderer>(
    screencopy: &ScreencopyFrame,
    damage_tracker: &'a mut Option<OutputDamageTracker>,
    renderer: &mut R,
    output_elements_result: &OutputElementsResult<R>,
) -> anyhow::Result<(Option<SyncPoint>, Option<&'a Vec<Rectangle<i32, Physical>>>)>
where
    FhtRenderElement<R>: RenderElement<R>,
{
    let output = screencopy.output();
    let transform = output.current_transform();
    let scale = Scale::from(output.current_scale().integer_scale() as f64); // TODO: Fractional scale support
    let output_region =
        Rectangle::from_loc_and_size(Point::default(), output.current_mode().unwrap().size);
    let region = screencopy.physical_region();

    let _ = damage_tracker.take_if(|dt| {
        let OutputModeSource::Static {
            size: last_size,
            scale: last_scale,
            transform: last_transform,
        } = dt.mode()
        else {
            unreachable!()
        };

        *last_size != output_region.size || *last_scale != scale || *last_transform != transform
    });
    let damage_tracker = damage_tracker
        .get_or_insert_with(|| OutputDamageTracker::new(output_region.size, scale, transform));

    let elements = match screencopy.overlay_cursor() {
        true => &output_elements_result.elements,
        false => &output_elements_result.elements[output_elements_result.cursor_elements_len..],
    };

    let (damage, _) = damage_tracker.damage_output(1, elements)?;
    if screencopy.with_damage() && damage.is_none() {
        return Ok((None, None));
    }
    let elements = elements.iter().rev();

    match screencopy.buffer() {
        ScreencopyBuffer::Shm(buffer) => {
            // We cannot render into shm buffer directly.
            // Instead we render inside a texture and export memory from framebuffer.
            let (_, _) = render_to_texture(
                renderer,
                output_region.size,
                scale,
                transform,
                Fourcc::Xrgb8888,
                elements,
            )?;

            let mapping = renderer.copy_framebuffer(
                region.to_logical(1).to_buffer(
                    1,
                    Transform::Normal,
                    &region.size.to_f64().to_logical(scale).to_i32_round(),
                ),
                Fourcc::Xrgb8888,
            )?;
            let pixels = renderer.map_texture(&mapping)?;

            with_buffer_contents_mut(buffer, |shm_ptr, shm_len, buffer_data| unsafe {
                anyhow::ensure!(
                    // The buffer prefers pixels in little endian ...
                    buffer_data.format == wl_shm::Format::Xrgb8888
                        && buffer_data.width == region.size.w
                        && buffer_data.height == region.size.h
                        && buffer_data.stride == region.size.w * 4
                        && shm_len == (buffer_data.stride * buffer_data.height) as usize,
                    "invalid buffer format or size"
                );

                {
                    crate::profile_scope!("copy_nonoverlapping_to_shm");
                    std::ptr::copy_nonoverlapping(pixels.as_ptr(), shm_ptr.cast(), shm_len);
                }
                Ok(())
            })??;

            // NOTE: for shm since its a memory copy we dont have to wait.
            Ok((None, damage))
        }
        ScreencopyBuffer::Dma(dmabuf) => {
            anyhow::ensure!(
                dmabuf.width() == region.size.w as u32
                    && dmabuf.height() == region.size.h as u32
                    && dmabuf.format().code == Fourcc::Xrgb8888,
                "Invalid dmabuf!"
            );

            if region == output_region {
                // Little optimization:
                // When our screencopy region is the same as the output region, we can render inside
                // the dmabuf directly.
                renderer.bind(dmabuf.clone())?;
                let sync_point =
                    render_elements(renderer, output_region.size, scale, transform, elements)?;
                renderer.unbind()?;

                Ok((Some(sync_point), damage))
            } else {
                // Otherwise we can blit inside the dmabuf
                let (_, _) = render_to_texture(
                    renderer,
                    output_region.size,
                    scale,
                    transform,
                    Fourcc::Xbgr8888,
                    elements,
                )?;

                renderer.blit_to(
                    dmabuf.clone(),
                    region,
                    Rectangle::from_loc_and_size(Point::default(), region.size),
                    TextureFilter::Linear,
                )?;

                Ok((None, damage))
            }
        }
    }
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
