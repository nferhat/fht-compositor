// Dual kawase blur, ported from niri (src/render_helpers/blur.rs).
use std::borrow::BorrowMut;
use std::cell::RefCell;
use std::rc::Rc;

use fht_compositor_config::{Blur as BlurConfig, BlurKind, LiquidGlass};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::{Element, Id, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    ffi, link_program, GlesError, GlesFrame, GlesRenderer, GlesTexture, Uniform,
};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Frame, FrameContext as _, Offscreen, Texture as _};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Size, Transform};

#[cfg(feature = "udev-backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};
use crate::renderer::shaders::Shaders;

#[derive(Debug, Clone, Copy)]
pub struct BlurProgram {
    down: BlurPass,
    up: BlurPass,
}

#[derive(Debug, Clone, Copy)]
struct BlurPass {
    program: ffi::types::GLuint,
    uniform_tex: ffi::types::GLint,
    uniform_half_pixel: ffi::types::GLint,
    uniform_offset: ffi::types::GLint,
    uniform_uv_scale: ffi::types::GLint,
    attrib_vert: ffi::types::GLint,
}

impl BlurPass {
    unsafe fn compile(gl: &ffi::Gles2, frag_src: &str) -> Result<Self, GlesError> {
        unsafe {
            let program = link_program(gl, include_str!("shaders/blur.vert"), frag_src)?;

            Ok(Self {
                program,
                uniform_tex: gl.GetUniformLocation(program, c"tex".as_ptr()),
                uniform_half_pixel: gl.GetUniformLocation(program, c"half_pixel".as_ptr()),
                uniform_offset: gl.GetUniformLocation(program, c"offset".as_ptr()),
                uniform_uv_scale: gl.GetUniformLocation(program, c"uv_scale".as_ptr()),
                attrib_vert: gl.GetAttribLocation(program, c"vert".as_ptr()),
            })
        }
    }
}

impl BlurProgram {
    pub fn compile(renderer: &mut GlesRenderer) -> Result<Self, GlesError> {
        renderer.with_context(|gl| unsafe {
            Ok(Self {
                down: BlurPass::compile(gl, include_str!("shaders/blur-down.frag"))?,
                up: BlurPass::compile(gl, include_str!("shaders/blur-up.frag"))?,
            })
        })?
    }
}

/// Blur state of a [`Tile`](crate::space::Tile).
///
/// The textures hold the captured backdrop of `area`, and the blurred result after each capture.
/// They are only reallocated when the captured area changes size.
#[derive(Debug)]
pub struct Blur {
    id: Id,
    /// The two textures the blur passes ping-pong between, `textures[0]` holds the result.
    textures: Option<[GlesTexture; 2]>,
    /// The output area the textures were captured from.
    area: Rectangle<i32, Physical>,
}

impl Blur {
    pub fn new() -> Self {
        Self {
            id: Id::new(),
            textures: None,
            area: Rectangle::default(),
        }
    }

    fn ensure_textures(
        &mut self,
        renderer: &mut GlesRenderer,
        size: Size<i32, Buffer>,
    ) -> Result<(), GlesError> {
        if self.textures.as_ref().is_none_or(|t| t[0].size() != size) {
            let first = Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, size)?;
            let second = Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, size)?;
            self.textures = Some([first, second]);
        }

        Ok(())
    }
}

/// Live blur of a set of physical rects.
///
/// This is a framebuffer effect: smithay calls [`RenderElement::capture_framebuffer`] before
/// [`RenderElement::draw`] whenever what is below the element changed. The capture copies the
/// backdrop into the [`Blur`] state and runs the blur passes on it.
#[derive(Debug)]
pub struct BlurElement {
    id: Id,
    /// Physical rects to blur, in output coordinates.
    rects: Vec<Rectangle<i32, Physical>>,
    /// Bounding box of `rects` expanded by the blur reach, this is the area that gets captured.
    geometry: Rectangle<i32, Physical>,
    /// Bounding box of `rects`, the corners are rounded along it.
    bounds: Rectangle<i32, Physical>,
    /// Rounding of the corners of `bounds`, in physical pixels, matching the window.
    corner_radius: f32,
    corner_power: f32,
    /// Changes whenever the region does, so the element is damaged.
    commit: CommitCounter,
    config: BlurConfig,
    state: Rc<RefCell<Blur>>,
}

impl BlurElement {
    pub fn new(
        rects: Vec<Rectangle<i32, Physical>>,
        config: BlurConfig,
        state: Rc<RefCell<Blur>>,
        corner_radius: f32,
        corner_power: f32,
        commit: CommitCounter,
    ) -> Self {
        let id = RefCell::borrow(&state).id.clone();

        // expand in order to properly sample around the areas.
        let margin = (2f32.powi(config.passes as i32 + 1) * config.radius).ceil() as i32;
        let expanded = rects
            .iter()
            .map(|rect| {
                let mut expanded = *rect;
                expanded.loc -= (margin, margin).into();
                expanded.size += (margin * 2, margin * 2).into();
                expanded
            })
            .reduce(|a, b| a.merge(b))
            .unwrap_or_default();

        // HACK: Snap the area to the output's grid, so the grid does not move with the window.
        //
        // The downsampled levels are sampled on a grid that starts at the captured area, so without
        // snapping, they would move with the window (on a pixel level), instead of relative to the
        // passes.
        //
        // This does mean that you lose on blur reactivity ever so slightly, but I'd rather have
        // stability over something that has box-blur like artifacts, ew.
        let grid = 1 << config.passes.max(1);
        let left = expanded.loc.x.div_euclid(grid) * grid;
        let top = expanded.loc.y.div_euclid(grid) * grid;
        let right = -(-(expanded.loc.x + expanded.size.w)).div_euclid(grid) * grid;
        let bottom = -(-(expanded.loc.y + expanded.size.h)).div_euclid(grid) * grid;
        let geometry = Rectangle::from_extremities((left, top), (right, bottom));
        let bounds = rects
            .iter()
            .copied()
            .reduce(|a, b| a.merge(b))
            .unwrap_or_default();
        Self {
            id,
            rects,
            geometry,
            bounds,
            corner_radius,
            corner_power,
            commit,
            config,
            state,
        }
    }
}

impl Element for BlurElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::new((0., 0.).into(), (1., 1.).into())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometry
    }

    fn is_framebuffer_effect(&self) -> bool {
        true
    }
}

impl RenderElement<GlowRenderer> for BlurElement {
    fn draw(
        &self,
        frame: &mut GlowFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let state = RefCell::borrow(&self.state);
        let Some(textures) = state.textures.as_ref() else {
            return Ok(());
        };

        let gles: &mut GlesFrame = frame.borrow_mut();
        let program = Shaders::get_from_frame(gles).blur_post.clone();
        let glass_enabled = f32::from(self.config.kind.is_some());
        let glass = match self.config.kind {
            Some(BlurKind::LiquidGlass(glass)) => glass,
            None => LiquidGlass::default(),
        };
        // Without a kind the tone is neutral, so only the rounding is applied.
        let (brightness, tint_color) = if glass_enabled > 0.0 {
            (glass.brightness, glass.tint_color)
        } else {
            (1.0, [0.0; 4])
        };
        let tex_size = textures[0].size();
        let (tex_w, tex_h) = (tex_size.w as f32, tex_size.h as f32);
        // dst doesn't get changed by RelocateRenderElement which is used for the workspace switch
        // so we have to account for it here... annoying.
        let offset = dst.loc - self.geometry.loc;
        let bounds_loc = self.bounds.loc + offset - state.area.loc;
        let bounds_size = self.bounds.size;

        let mut res = Ok(());
        for rect in &self.rects {
            let rect = Rectangle::new(rect.loc + offset, rect.size);
            let Some(visible) = rect.intersection(state.area) else {
                continue;
            };

            // The damage is relative to `dst`. Only repaint the damaged part, the rest is still
            // showing the elements above, painting over them would leave splotches.
            let visible_damage: Vec<_> = damage
                .iter()
                .filter_map(|d| Rectangle::new(d.loc + dst.loc, d.size).intersection(visible))
                .map(|d| Rectangle::new(d.loc - visible.loc, d.size))
                .collect();
            if visible_damage.is_empty() {
                continue;
            }

            let loc = visible.loc - state.area.loc;
            let src = Rectangle::<f64, Buffer>::new(
                (loc.x as f64, loc.y as f64).into(),
                (visible.size.w as f64, visible.size.h as f64).into(),
            );

            // Every blurred rect is rounded like the window, the glass and the tone come on top.
            let uniforms = vec![
                Uniform::new("geo_size", (bounds_size.w as f32, bounds_size.h as f32)),
                Uniform::new(
                    "rect_origin",
                    (bounds_loc.x as f32 / tex_w, bounds_loc.y as f32 / tex_h),
                ),
                Uniform::new(
                    "rect_size",
                    (bounds_size.w as f32 / tex_w, bounds_size.h as f32 / tex_h),
                ),
                Uniform::new("corner_radius", self.corner_radius),
                Uniform::new("corner_power", self.corner_power),
                Uniform::new("glass_enabled", glass_enabled),
                Uniform::new("edge_thickness", glass.edge_thickness),
                Uniform::new("refraction_strength", glass.refraction_strength),
                Uniform::new("lens_distortion", glass.lens_distortion),
                Uniform::new("chromatic_aberration", glass.chromatic_aberration),
                Uniform::new("fresnel_strength", glass.fresnel_strength),
                Uniform::new("specular_strength", glass.specular_strength),
                Uniform::new("glass_opacity", glass.glass_opacity),
                Uniform::new("brightness", brightness),
                Uniform::new("contrast", self.config.contrast),
                Uniform::new("saturation", self.config.saturation),
                Uniform::new("vibrancy", self.config.vibrancy),
                Uniform::new("dark_vibrancy", self.config.dark_vibrancy),
                Uniform::new(
                    "tint_color",
                    (tint_color[0], tint_color[1], tint_color[2], tint_color[3]),
                ),
            ];
            gles.override_default_tex_program(program.clone(), uniforms);
            res = Frame::render_texture_from_to(
                gles,
                &textures[0],
                src,
                visible,
                &visible_damage,
                &[],
                Transform::Normal,
                1.0,
            );
            if res.is_err() {
                break;
            }
        }

        // Never leave the override behind, the next element would be drawn with it.
        gles.clear_tex_program_override();
        res
    }

    fn underlying_storage(&self, _renderer: &mut GlowRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }

    fn capture_framebuffer(
        &self,
        frame: &mut GlowFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        _cache: &UserDataMap,
    ) -> Result<(), GlesError> {
        // Capture where we're drawn, not where we were built, see `draw`.
        let output_rect = Rectangle::from_size(frame.output_size());
        let Some(area) = dst.intersection(output_rect) else {
            return Ok(());
        };

        let frame: &mut GlesFrame = frame.borrow_mut();
        let span = smithay::gpu_span_location!("BlurElement::capture_framebuffer");
        frame.with_gpu_span(span, |frame| {
            let program = Shaders::get_from_frame(frame).blur;

            let mut state = RefCell::borrow_mut(&self.state);
            // NOTE: Dropping the frame's renderer guard rebinds the frame's target and restores
            // its viewport, scissor and blend state, so the passes can run through it.
            let mut guard = frame.renderer();
            let renderer = guard.as_mut();

            state.ensure_textures(renderer, Size::from((area.size.w, area.size.h)))?;
            state.area = area;
            let Some(textures) = state.textures.as_ref() else {
                return Ok(());
            };

            renderer.with_context(|gl| unsafe {
                // We first need to capture the region inside the framebuffer
                capture_region(gl, area, &textures[0]);

                blur_in_place(
                    gl,
                    program,
                    textures,
                    self.config.passes,
                    self.config.radius,
                );
            })?;

            Ok(())
        })
    }
}

#[cfg(feature = "udev-backend")]
impl<'render> RenderElement<UdevRenderer<'render>> for BlurElement {
    fn draw(
        &self,
        frame: &mut UdevFrame<'render, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), UdevRenderError> {
        <Self as RenderElement<GlowRenderer>>::draw(
            self,
            frame.as_mut(),
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )
        .map_err(UdevRenderError::Render)
    }

    fn underlying_storage(&self, _: &mut UdevRenderer<'render>) -> Option<UnderlyingStorage<'_>> {
        None
    }

    fn capture_framebuffer(
        &self,
        frame: &mut UdevFrame<'render, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), UdevRenderError> {
        <Self as RenderElement<GlowRenderer>>::capture_framebuffer(
            self,
            frame.as_mut(),
            src,
            dst,
            cache,
        )
        .map_err(UdevRenderError::Render)
    }
}

/// Copy `area` of the bound draw framebuffer into `texture`.
///
/// Logical rows match framebuffer rows here (smithay's projection already accounts for GL's
/// orientation), so the blit does not flip anything.
unsafe fn capture_region(gl: &ffi::Gles2, area: Rectangle<i32, Physical>, texture: &GlesTexture) {
    unsafe {
        let mut current = 0;
        gl.GetIntegerv(ffi::DRAW_FRAMEBUFFER_BINDING, &mut current);

        let mut fbo = 0;
        gl.GenFramebuffers(1, &mut fbo);
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, current as u32);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);
        gl.FramebufferTexture2D(
            ffi::DRAW_FRAMEBUFFER,
            ffi::COLOR_ATTACHMENT0,
            ffi::TEXTURE_2D,
            texture.tex_id(),
            0,
        );

        // BlitFramebuffer is affected by the scissor test.
        gl.Disable(ffi::SCISSOR_TEST);
        gl.BlitFramebuffer(
            area.loc.x,
            area.loc.y,
            area.loc.x + area.size.w,
            area.loc.y + area.size.h,
            0,
            0,
            area.size.w,
            area.size.h,
            ffi::COLOR_BUFFER_BIT,
            ffi::LINEAR,
        );

        gl.DeleteFramebuffers(1, &fbo);
    }
}

/// Blur the captured backdrop in `textures[0]` in place, `textures[1]` is the scratch texture.
///
/// Both textures are the size of the captured area. Level `k` is `size >> k` and lives in
/// `textures[k % 2]`, it is sampled through the part of the texture it covers.
unsafe fn blur_in_place(
    gl: &ffi::Gles2,
    program: BlurProgram,
    textures: &[GlesTexture; 2],
    passes: usize,
    offset: f32,
) {
    const VERTICES: [f32; 12] = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0];

    let passes = passes.max(1);
    let tex_size = textures[0].size();

    let level_size =
        |k: usize| Size::<i32, Buffer>::from(((tex_size.w >> k).max(1), (tex_size.h >> k).max(1)));
    let tex = |k: usize| &textures[k % 2];
    // The part of the texture that holds level `k`, in texture uv.
    let uv_scale = |k: usize| {
        let size = level_size(k);
        (
            size.w as f32 / tex_size.w as f32,
            size.h as f32 / tex_size.h as f32,
        )
    };
    // One level pixel is one texel of the texture, half of it is the sampling step.
    let half_pixel = (0.5 / tex_size.w as f32, 0.5 / tex_size.h as f32);

    // Draws level `dst` from level `src` with `pass`, sampling `src` through its part of the
    // texture.
    let draw = |pass: &BlurPass, src: usize, dst: usize| unsafe {
        gl.UseProgram(pass.program);
        gl.Uniform1i(pass.uniform_tex, 0);
        gl.Uniform1f(pass.uniform_offset, offset);
        gl.Uniform2f(pass.uniform_half_pixel, half_pixel.0, half_pixel.1);
        gl.Uniform2f(pass.uniform_uv_scale, uv_scale(src).0, uv_scale(src).1);

        let dst_size = level_size(dst);
        gl.Viewport(0, 0, dst_size.w, dst_size.h);
        gl.FramebufferTexture2D(
            ffi::DRAW_FRAMEBUFFER,
            ffi::COLOR_ATTACHMENT0,
            ffi::TEXTURE_2D,
            tex(dst).tex_id(),
            0,
        );

        gl.BindTexture(ffi::TEXTURE_2D, tex(src).tex_id());
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
        gl.TexParameteri(
            ffi::TEXTURE_2D,
            ffi::TEXTURE_WRAP_S,
            ffi::CLAMP_TO_EDGE as i32,
        );
        gl.TexParameteri(
            ffi::TEXTURE_2D,
            ffi::TEXTURE_WRAP_T,
            ffi::CLAMP_TO_EDGE as i32,
        );

        let attrib = pass.attrib_vert as u32;
        gl.EnableVertexAttribArray(attrib);
        gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
        gl.VertexAttribPointer(
            attrib,
            2,
            ffi::FLOAT,
            ffi::FALSE,
            0,
            VERTICES.as_ptr().cast(),
        );
        gl.DrawArrays(ffi::TRIANGLES, 0, 6);
        gl.DisableVertexAttribArray(attrib);
    };

    gl.Disable(ffi::BLEND);
    gl.Disable(ffi::SCISSOR_TEST);
    gl.ActiveTexture(ffi::TEXTURE0);

    let mut fbo = 0;
    gl.GenFramebuffers(1, &mut fbo);
    gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);

    // Downsample: level k - 1 -> level k
    for k in 1..=passes {
        draw(&program.down, k - 1, k);
    }

    // Upsample: level k -> level k - 1
    for k in (1..=passes).rev() {
        draw(&program.up, k, k - 1);
    }

    gl.DeleteFramebuffers(1, &fbo);
}
