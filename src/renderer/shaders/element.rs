//! A re-implementation of Smithay's [`PixelShaderElement`] that allows it to change opacity on the
//! fly, which is required for some animations in the compositor.

// FIXME: In the future, maybe adding the possibility to attach textures to pixel shader elements
// would be quite an interesting opportunity for effects and transitions, but for now, using custom
// texture shaders is enough.

use std::borrow::BorrowMut;

use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, GlesPixelProgram, Uniform};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::utils::{Logical, Physical, Rectangle, Scale, Transform};

#[cfg(feature = "udev-backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};

#[derive(Debug, Clone)]
pub struct ShaderElement {
    shader: GlesPixelProgram,
    id: Id,
    commit_counter: CommitCounter,
    area: Rectangle<i32, Logical>,
    opaque_regions: Vec<Rectangle<i32, Logical>>,
    alpha: f32,
    additional_uniforms: Vec<Uniform<'static>>,
    kind: Kind,
}

#[allow(unused)] // will need rework of decorations before we actually use resize and set_alpha
impl ShaderElement {
    pub fn new(
        shader: GlesPixelProgram,
        area: Rectangle<i32, Logical>,
        opaque_regions: Option<Vec<Rectangle<i32, Logical>>>,
        alpha: f32,
        additional_uniforms: Vec<Uniform<'_>>,
        kind: Kind,
    ) -> Self {
        ShaderElement {
            shader,
            id: Id::new(),
            commit_counter: CommitCounter::default(),
            area,
            opaque_regions: opaque_regions.unwrap_or_default(),
            alpha,
            additional_uniforms: additional_uniforms
                .into_iter()
                .map(|u| u.into_owned())
                .collect(),
            kind,
        }
    }

    /// Set the alpha value.
    pub fn set_alpha(&mut self, alpha: f32) {
        if self.alpha != alpha {
            if alpha < 1.0 {
                // no opaque regions not fully opaque
                self.opaque_regions.clear();
            }
            self.alpha = alpha;
            self.commit_counter.increment();
        }
    }

    /// Resize the canvas area
    pub fn resize(
        &mut self,
        area: Rectangle<i32, Logical>,
        opaque_regions: Option<Vec<Rectangle<i32, Logical>>>,
    ) {
        let opaque_regions = opaque_regions.unwrap_or_default();
        if self.area != area || self.opaque_regions != opaque_regions {
            self.area = area;

            if self.alpha < 1.0 {
                // no opaque regions not fully opaque
                self.opaque_regions.clear();
            } else {
                self.opaque_regions = opaque_regions;
            }

            self.commit_counter.increment();
        }
    }

    /// Update the additional uniforms
    /// (see [`GlesRenderer::compile_custom_pixel_shader`] and
    /// [`GlesFrame::render_pixel_shader_to`]).
    ///
    /// This replaces the stored uniforms, you have to update all of them, partial updates are not
    /// possible.
    pub fn update_uniforms(&mut self, additional_uniforms: Vec<Uniform<'_>>) {
        self.additional_uniforms = additional_uniforms
            .into_iter()
            .map(|u| u.into_owned())
            .collect();
        self.commit_counter.increment();
    }
}

impl Element for ShaderElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit_counter
    }

    fn src(&self) -> Rectangle<f64, smithay::utils::Buffer> {
        Rectangle::from_size(self.area.size.to_f64().to_buffer(1.0, Transform::Normal))
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.area.to_physical_precise_round(scale)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.opaque_regions
            .iter()
            .map(|region| region.to_physical_precise_round(scale))
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        self.kind
    }
}

impl RenderElement<GlowRenderer> for ShaderElement {
    fn draw(
        &self,
        frame: &mut GlowFrame,
        src: Rectangle<f64, smithay::utils::Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        crate::profile_function!();
        let gles_frame: &mut GlesFrame = frame.borrow_mut();
        gles_frame.render_pixel_shader_to(
            &self.shader,
            src,
            dst,
            self.area.size.to_buffer(1, Transform::Normal),
            Some(damage),
            self.alpha,
            &self.additional_uniforms,
        )
    }
}

#[cfg(feature = "udev-backend")]
impl<'a> RenderElement<UdevRenderer<'a>> for ShaderElement {
    fn draw(
        &self,
        frame: &mut UdevFrame<'a, '_, '_>,
        src: Rectangle<f64, smithay::utils::Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), UdevRenderError> {
        crate::profile_function!();
        let glow_frame = frame.as_mut();
        let gles_frame: &mut GlesFrame = glow_frame.borrow_mut();
        gles_frame
            .render_pixel_shader_to(
                &self.shader,
                src,
                dst,
                self.area.size.to_buffer(1, Transform::Normal),
                Some(damage),
                self.alpha,
                &self.additional_uniforms,
            )
            .map_err(UdevRenderError::Render)
    }
}
