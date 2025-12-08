//! The renderer part of fht-compositor.
//!
//! The renderer is the part responsible for drawing elements. In the smithay pipeline, this is
//! creating the various [`RenderElement`]s that are getting them submitted to a buffer attached to
//! an output that the renderer can bind to.
//!
//! This module also has some helpers to create render elements.

pub mod blur;
mod data;
pub mod extra_damage;
pub mod render_elements;
pub mod rounded_window;
pub mod shaders;
pub mod texture_element;
pub mod texture_shader_element;

use std::borrow::BorrowMut;

use anyhow::Context;
use blur::EffectsFramebuffers;
use glam::{Mat3, Vec2};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Buffer as _, Fourcc};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::utils::RelocateRenderElement;
use smithay::backend::renderer::element::{AsRenderElements, RenderElement};
use smithay::backend::renderer::gles::{
    GlesError, GlesMapping, GlesTexture, Uniform, UniformValue,
};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
use smithay::backend::renderer::{
    Bind, Blit, Color32F, ExportMem, Frame, ImportAll, ImportMem, Offscreen, Renderer,
    RendererSuper, Texture, TextureFilter, TextureMapping,
};
use smithay::desktop::layer_map_for_output;
use smithay::desktop::space::SurfaceTree;
use smithay::input::pointer::CursorImageStatus;
use smithay::output::{Output, OutputModeSource};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, Mode, PostAction};
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{
    Buffer, IsAlive, Logical, Physical, Point, Rectangle, Scale, Size, Transform,
};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::wlr_layer::Layer;
use smithay::wayland::shm::with_buffer_contents_mut;

use crate::config::ui::ConfigUiRenderElement;
use crate::cursor::CursorRenderElement;
use crate::handlers::session_lock::SessionLockRenderElement;
use crate::layer::LayerShellRenderElement;
use crate::protocols::screencopy::{ScreencopyBuffer, ScreencopyFrame};
use crate::space::{MonitorRenderElement, MonitorRenderResult, TileRenderElement};
use crate::state::Fht;
use crate::utils::get_monotonic_time;

crate::fht_render_elements! {
    FhtRenderElement<R> => {
        Cursor = CursorRenderElement<R>,
        ConfigUi = ConfigUiRenderElement,
        Monitor = MonitorRenderElement<R>,
        InteractiveSwapTile = RelocateRenderElement<TileRenderElement<R>>,
        LayerShell = LayerShellRenderElement<R>,
        SessionLock = SessionLockRenderElement<R>,
        Debug = DebugRenderElement,
    }
}

crate::fht_render_elements! {
    DebugRenderElement => {
        Solid = SolidColorRenderElement,
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
    ) -> OutputElementsResult<R> {
        crate::profile_function!();
        let active_output = self.space.active_output();
        // Yes, we do not support fractional scale.
        //
        // For now, the ecosystem in wayland has all the required protocols and stuff to support it
        // properly, but still, to this day, client support is very lacking, ranging from works
        // very fine to absolute garbage.
        //
        // When the Wayland space will see evolutions regarding fractional scaling, I'll reconsider
        // this choice and support it. But as far as I can see, this isn't happening.
        let scale = output.current_scale().integer_scale();

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
                1.0,
                self.clock.now().into(),
            ) {
                rv.cursor_elements_len += elements.len();
                rv.elements.extend(elements.into_iter().map(Into::into));
            }

            // Draw drag and drop icon.
            if let Some(surface) = self.dnd_icon.as_ref().filter(IsAlive::alive) {
                let elements = AsRenderElements::<R>::render_elements::<CursorRenderElement<R>>(
                    &SurfaceTree::from_surface(surface),
                    renderer,
                    cursor_element_pos,
                    Scale::from(scale as f64),
                    1.0,
                );
                rv.cursor_elements_len += elements.len();
                rv.elements.extend(elements.into_iter().map(Into::into));
            }
        }

        if !self.config_ui.hidden() {
            // Draw config ui below cursor, only if we didnt start drawing it on another output.
            let config_ui_output = self
                .config_ui_output
                .get_or_insert_with(|| self.space.active_output().clone());
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
            let elements = self.session_lock_elements(renderer, output);
            rv.elements.extend(elements.into_iter().map(Into::into));
        }

        // Collect the render elements for the rendered monitor
        let monitor = self.space.monitor_mut_for_output(output).unwrap();
        let has_blur = monitor.has_blur();
        let MonitorRenderResult {
            elements: monitor_elements,
            elements_above_top: monitor_elements_above_top,
        } = monitor.render(renderer, scale);
        // And interactive swap elements
        let interactive_move_elements = self.space.render_interactive_swap(renderer, output, scale);

        let layer_map = layer_map_for_output(output);
        let mut extend_from_layer = |elements: &mut Vec<FhtRenderElement<R>>, layer| {
            for mapped in layer_map
                .layers_on(layer)
                .filter_map(|layer| self.mapped_layer_surfaces.get(layer))
                .rev()
            {
                let layer_geo = layer_map.layer_geometry(&mapped.layer).unwrap();
                elements.extend(
                    mapped
                        .render(renderer, layer_geo, scale, &self.config)
                        .map(Into::into)
                        .collect::<Vec<_>>(),
                );
            }
        };

        // Overlay layer shells are drawn above everything else, including fullscreen windows
        extend_from_layer(&mut rv.elements, Layer::Overlay);
        // Then we collect the top layer shells, which might be rendered above or below the
        // monitor contents depending on the currently displayed workspace.
        let mut top = vec![];
        extend_from_layer(&mut top, Layer::Top);
        // Then the background layer shells that are always behind
        let mut background = vec![];
        extend_from_layer(&mut background, Layer::Bottom);
        extend_from_layer(&mut background, Layer::Background);

        // The tile we grab is always rendered above everything else.
        rv.elements
            .extend(interactive_move_elements.into_iter().map(Into::into));
        // First elements that should be rendered above the top layer shell. We do this since there
        // is a potential case where we switch between two workspaces where one has fullscreened
        // tile and the other dont.
        rv.elements
            .extend(monitor_elements_above_top.into_iter().map(Into::into));
        // Then the top layer shells
        rv.elements.extend(top);
        // The content that should be below the top layer shells
        rv.elements
            .extend(monitor_elements.into_iter().map(Into::into));
        // And finally the rest of the layer shells
        rv.elements.extend(background);

        // We don't need it anymore, and avoid deadlock down below.
        drop(layer_map);

        // In case the optimized blur layer is dirty, re-render
        // It only has the bottom and background layer shells drawn onto with blur applied.
        //
        // We must do it now before we actually render the previous render elements into the final
        // composited blur buffer
        let mut fx_buffers = EffectsFramebuffers::get(output);
        if !self.config.decorations.blur.disable
            && self.config.decorations.blur.passes > 0
            && fx_buffers.optimized_blur_dirty
            && has_blur
        {
            if let Err(err) = fx_buffers.update_optimized_blur_buffer(
                renderer.glow_renderer_mut(),
                output,
                scale,
                &self.config.decorations.blur,
            ) {
                error!(?err, "Failed to update optimized blur buffer");
            } else {
                fx_buffers.optimized_blur_dirty = false;
            }
        }

        rv
    }

    #[cfg(feature = "xdg-screencast-portal")]
    pub fn render_screencast<R: FhtRenderer>(
        &mut self,
        output: &Output,
        renderer: &mut R,
        output_elements_result: &OutputElementsResult<R>,
    ) where
        FhtRenderElement<R>: RenderElement<R>,
    {
        // NOTE: For output screencasts there's no need to send frame callbacks since this is called
        // right before [`Fht::send_frames`]
        crate::profile_function!();
        use crate::utils::pipewire::CastSource;

        let size = output.current_mode().unwrap().size;
        let transform = output.current_transform();
        let size = transform.transform_size(size);
        let scale = output.current_scale().integer_scale();
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
            crate::profile_scope!("render_cast", &cast.id().to_string());
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

            if let Err(err) =
                cast.render_for_output(renderer, output_elements_result, size, scale as f64)
            {
                error!(id = ?cast.id(), ?err, "Failed to render cast");
            }
        }
        pipewire.casts = casts;

        for id in casts_to_stop {
            self.stop_cast(id);
        }
    }

    #[cfg(feature = "xdg-screencast-portal")]
    pub fn render_screencast_windows<R: FhtRenderer>(
        &mut self,
        output: &Output,
        renderer: &mut R,
        target_presentation_time: std::time::Duration,
    ) where
        smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<R>:
            RenderElement<R>,
    {
        crate::profile_function!();

        use crate::state::send_frame_for_screencast_window;
        use crate::utils::pipewire::CastSource;

        let scale: Scale<f64> = (output.current_scale().integer_scale() as f64).into();

        let Some(pipewire) = self.pipewire.as_mut() else {
            return;
        };

        if pipewire.casts.is_empty() {
            return;
        }

        let windows = self.space.windows_on_output(output).collect::<Vec<_>>();
        let visible_windows = self
            .space
            .visible_windows_for_output(output)
            .collect::<Vec<_>>();
        let mut casts = std::mem::take(&mut pipewire.casts);
        let mut casts_to_stop = vec![];

        for cast in &mut casts {
            crate::profile_scope!("render_cast", &cast.id().to_string());

            if !cast.active() {
                trace!(id = ?cast.id(), "Cast is not active, skipping");
                continue;
            }

            let CastSource::Window(weak_window) = cast.source() else {
                continue;
            };
            let Some(window) = weak_window.upgrade() else {
                continue;
            };

            if !windows.iter().any(|w| **w == window) {
                continue;
            }
            if !visible_windows.iter().any(|w| **w == window) {
                send_frame_for_screencast_window(
                    output,
                    &self.output_state,
                    &window,
                    target_presentation_time,
                );
            }

            let bbox = window.bbox_with_popups().to_physical_precise_up(scale);
            match cast.ensure_size(bbox.size) {
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

            let loc = window.render_offset().to_physical_precise_round(scale) - bbox.loc;
            let mut elements = window.render_toplevel_elements(renderer, loc, scale, 1.);
            elements.extend(window.render_popup_elements(renderer, loc, scale, 1.));

            if let Err(err) = cast.render(renderer, &elements, bbox.size, scale) {
                error!(id = ?cast.id(), ?err, "Failed to render cast");
            }
        }
        pipewire.casts = casts;

        for id in casts_to_stop {
            self.stop_cast(id);
        }
    }

    #[cfg(feature = "xdg-screencast-portal")]
    pub fn render_screencast_workspaces<R: FhtRenderer>(
        &mut self,
        output: &Output,
        renderer: &mut R,
        target_presentation_time: std::time::Duration,
    ) where
        crate::space::WorkspaceRenderElement<R>: RenderElement<R>,
    {
        crate::profile_function!();

        use crate::state::send_frame_for_screencast_window;
        use crate::utils::pipewire::CastSource;

        let scale = output.current_scale().integer_scale();
        let size = output.current_mode().unwrap().size;
        let transform = output.current_transform();
        let size = transform.transform_size(size);
        let mon = self.space.monitor_mut_for_output(output).unwrap();

        let Some(pipewire) = self.pipewire.as_mut() else {
            return;
        };

        if pipewire.casts.is_empty() {
            return;
        }

        let mut casts = std::mem::take(&mut pipewire.casts);
        let mut casts_to_stop = vec![];

        for cast in &mut casts {
            crate::profile_scope!("render_cast", &cast.id().to_string());

            if !cast.active() {
                trace!(id = ?cast.id(), "Cast is not active, skipping");
                continue;
            }

            let CastSource::Workspace {
                output: weak_output,
                index,
            } = cast.source()
            else {
                continue;
            };
            let index = *index;
            let Some(ws_output) = weak_output.upgrade() else {
                continue;
            };
            if ws_output != *output {
                return;
            }

            let ws = mon.workspace_by_index(index);
            if index != mon.active_workspace_idx() {
                let windows = ws.windows();

                if windows.len() == 0 {
                    // no need to bother
                    continue;
                }

                for window in ws.windows() {
                    send_frame_for_screencast_window(
                        &ws_output,
                        &self.output_state,
                        window,
                        target_presentation_time,
                    );
                }
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

            // NOTE: The workspace already renders to the origin (0, 0), so no need to relocate
            // anything.

            let elements =
                mon.workspace_mut_by_index(index)
                    .render(renderer, scale, Some(Point::default()));

            if let Err(err) = cast.render(renderer, &elements, size, scale as f64) {
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
    + RendererSuper
    + ImportAll
    + ImportMem
    + Bind<Dmabuf>
    + Blit
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

impl AsGlowRenderer for GlowRenderer {
    fn glow_renderer(&self) -> &GlowRenderer {
        self
    }

    fn glow_renderer_mut(&mut self) -> &mut GlowRenderer {
        self
    }
}

/// Inititalize needed structs and shaders for custom rendering.
pub fn init(renderer: &mut GlowRenderer) {
    shaders::Shaders::init(renderer);
    data::RendererData::init(renderer.borrow_mut());
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

    let mut texture = renderer
        .create_buffer(fourcc, buffer_size)
        .context("error creating texture")?;
    let mut fb = renderer
        .bind(&mut texture)
        .context("error binding texture")?;
    let sync_point = render_elements(renderer, &mut fb, size, scale, transform, elements)?;
    drop(fb);

    Ok((texture, sync_point))
}

/// Render the given `elements` inside the current bound target.
///
/// It is up to **YOU** to bind and unbind the renderer before and after calling this function.
pub fn render_elements<R: FhtRenderer>(
    renderer: &mut R,
    fb: &mut R::Framebuffer<'_>,
    size: Size<i32, Physical>,
    scale: impl Into<Scale<f64>>,
    transform: Transform,
    elements: impl Iterator<Item = impl RenderElement<R>>,
) -> anyhow::Result<SyncPoint> {
    let scale = scale.into();
    let transform = transform.invert();
    let frame_rect = Rectangle::from_size(transform.transform_size(size));
    let mut frame = renderer
        .render(fb, size, transform)
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

#[allow(clippy::type_complexity)]
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
    // See note in Fht::output_elements about fractional scale
    let scale = Scale::from(output.current_scale().integer_scale() as f64);
    let output_region = Rectangle::new(Point::default(), output.current_mode().unwrap().size);
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
            let (mut tex, _) = render_to_texture(
                renderer,
                output_region.size,
                scale,
                transform,
                Fourcc::Xrgb8888,
                elements,
            )?;

            let fb = renderer.bind(&mut tex)?;
            let mapping = renderer.copy_framebuffer(
                &fb,
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
                let mut dmabuf = dmabuf.clone();
                let mut fb = renderer.bind(&mut dmabuf)?;
                let sync_point = render_elements(
                    renderer,
                    &mut fb,
                    output_region.size,
                    scale,
                    transform,
                    elements,
                )?;
                drop(fb);

                Ok((Some(sync_point), damage))
            } else {
                // Otherwise we can blit inside the dmabuf
                let (mut tex, _) = render_to_texture(
                    renderer,
                    output_region.size,
                    scale,
                    transform,
                    Fourcc::Xbgr8888,
                    elements,
                )?;

                let mut dmabuf = dmabuf.clone();
                let tex_fb = renderer.bind(&mut tex)?;
                let mut dmabuf_fb = renderer.bind(&mut dmabuf)?;

                renderer
                    .blit(
                        &tex_fb,
                        &mut dmabuf_fb,
                        region,
                        Rectangle::new(Point::default(), region.size),
                        TextureFilter::Linear,
                    )?
                    .wait()?;

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

// Copied from smithay, adapted to use glam structs
fn build_texture_mat(
    src: Rectangle<f64, Buffer>,
    dest: Rectangle<i32, Physical>,
    texture: Size<i32, Buffer>,
    transform: Transform,
) -> Mat3 {
    let dst_src_size = transform.transform_size(src.size);
    let scale = dst_src_size.to_f64() / dest.size.to_f64();

    let mut tex_mat = Mat3::IDENTITY;
    // first bring the damage into src scale
    tex_mat = Mat3::from_scale(Vec2::new(scale.x as f32, scale.y as f32)) * tex_mat;

    // then compensate for the texture transform
    let transform_mat = Mat3::from_cols_array(transform.matrix().as_ref());
    let translation = match transform {
        Transform::Normal => Mat3::IDENTITY,
        Transform::_90 => Mat3::from_translation(Vec2::new(0f32, dst_src_size.w as f32)),
        Transform::_180 => {
            Mat3::from_translation(Vec2::new(dst_src_size.w as f32, dst_src_size.h as f32))
        }
        Transform::_270 => Mat3::from_translation(Vec2::new(dst_src_size.h as f32, 0f32)),
        Transform::Flipped => Mat3::from_translation(Vec2::new(dst_src_size.w as f32, 0f32)),
        Transform::Flipped90 => Mat3::IDENTITY,
        Transform::Flipped180 => Mat3::from_translation(Vec2::new(0f32, dst_src_size.h as f32)),
        Transform::Flipped270 => {
            Mat3::from_translation(Vec2::new(dst_src_size.h as f32, dst_src_size.w as f32))
        }
    };
    tex_mat = transform_mat * tex_mat;
    tex_mat = translation * tex_mat;

    // now we can add the src crop loc, the size already done implicit by the src size
    tex_mat = Mat3::from_translation(Vec2::new(src.loc.x as f32, src.loc.y as f32)) * tex_mat;

    // at last we have to normalize the values for UV space
    tex_mat = Mat3::from_scale(Vec2::new(
        (1.0f64 / texture.w as f64) as f32,
        (1.0f64 / texture.h as f64) as f32,
    )) * tex_mat;

    tex_mat
}

/// Get whether a surface has any transparent region. This is calculated from opaque regions
/// provided by the surface aswell as the render format.
pub fn has_transparent_region(surface: &WlSurface, surface_size: Size<i32, Logical>) -> bool {
    // Opaque regions are described in surface-local coordinates.
    let surface_geo = Rectangle::from_size(surface_size);
    with_states(surface, |data| {
        let renderer_data = data
            .data_map
            .get::<RendererSurfaceStateUserData>()
            .unwrap()
            .lock()
            .unwrap();
        if let Some(opaque_regions) = renderer_data.opaque_regions() {
            // If there's some place left after removing opaque regions, these are
            // transparent regions and must be rendered using blur.
            let remaining = surface_geo.subtract_rects(opaque_regions.iter().copied());
            !remaining.is_empty()
        } else {
            // no opaque regions == fully transparent window surface
            true
        }
    })
}
