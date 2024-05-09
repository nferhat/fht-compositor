use std::borrow::BorrowMut;

use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, GlesTexProgram, Uniform};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Transform};

use super::AsGlowFrame;
#[cfg(feature = "udev_backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};

/// A wrapper for any E to simplify overriding it with a custom texture shader if needed.
///
/// Check smithay's [`GlesRenderer::compile_custom_texture_shader`] documentation for information
/// about which uniforms are getting passed in.
///
/// In addition to this, the shader also gets passed in `size` (the dst size of the element)
#[derive(Debug)]
pub struct CustomTextureShaderElement<E: Element> {
    element: E,
    texture_shader: Option<(GlesTexProgram, Vec<Uniform<'static>>)>,
}

impl<E: Element> CustomTextureShaderElement<E> {
    /// Create a custom texture shader element with a custom texture shder.
    pub fn from_element(
        element: E,
        texture_shader: GlesTexProgram,
        uniforms: Vec<Uniform<'static>>,
    ) -> Self {
        Self {
            element,
            texture_shader: Some((texture_shader, uniforms)),
        }
    }

    /// Create a custom texture shader element without a custom texture shader.
    ///
    /// Useful if you want to avoid creating a whole other variant just to not apply a texture
    /// shader.
    pub fn from_element_no_shader(element: E) -> Self {
        Self {
            element,
            texture_shader: None,
        }
    }
}

impl<E: Element> Element for CustomTextureShaderElement<E> {
    fn id(&self) -> &Id {
        self.element.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.element.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.element.src()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.element.geometry(scale)
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.geometry(scale).loc
    }

    fn transform(&self) -> Transform {
        Transform::Normal
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> smithay::backend::renderer::utils::DamageSet<i32, Physical> {
        self.element.damage_since(scale, commit)
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        // TODO: We can't really do opaque regions at the moment since the texture shader may or
        // may no mess with what is opaque or not in the rendered texture.
        //
        // Maybe add a closure for the opaque region calc?
        //
        // self.element.opaque_regions(scale)
        vec![]
    }

    fn alpha(&self) -> f32 {
        self.element.alpha()
    }

    fn kind(&self) -> Kind {
        self.element.kind()
    }
}

impl<E> RenderElement<GlowRenderer> for CustomTextureShaderElement<E>
where
    E: Element, // base requirement for ^^^^^^^^^^^^
    E: RenderElement<GlowRenderer>,
{
    fn draw(
        &self,
        frame: &mut GlowFrame<'_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        if let Some((program, user_uniforms)) = self.texture_shader.clone() {
            // Override texture shader with our uniforms
            let gles_frame: &mut GlesFrame = BorrowMut::borrow_mut(frame);

            let mut additional_uniforms =
                vec![Uniform::new("size", [dst.size.w as f32, dst.size.h as f32])];
            additional_uniforms.extend(user_uniforms.clone());
            gles_frame.override_default_tex_program(program.clone(), additional_uniforms);

            let res = self.element.draw(frame, src, dst, damage);

            // Never forget to reset since its not our responsibility to manage texture shaders.
            BorrowMut::<GlesFrame>::borrow_mut(frame.glow_frame_mut()).clear_tex_program_override();

            res
        } else {
            self.element.draw(frame, src, dst, damage)
        }
    }

    fn underlying_storage(&self, renderer: &mut GlowRenderer) -> Option<UnderlyingStorage> {
        self.element.underlying_storage(renderer)
    }
}

#[cfg(feature = "udev_backend")]
impl<'a, E> RenderElement<UdevRenderer<'a>> for CustomTextureShaderElement<E>
where
    E: Element, // base requirement for ^^^^^^^^^^^^
    E: RenderElement<UdevRenderer<'a>>,
{
    fn draw(
        &self,
        frame: &mut UdevFrame<'a, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), UdevRenderError<'a>> {
        if let Some((program, user_uniforms)) = self.texture_shader.clone() {
            // Override texture shader with our uniforms
            let gles_frame: &mut GlesFrame = BorrowMut::borrow_mut(frame.glow_frame_mut());

            let mut additional_uniforms =
                vec![Uniform::new("size", [dst.size.w as f32, dst.size.h as f32])];
            additional_uniforms.extend(user_uniforms.clone());
            gles_frame.override_default_tex_program(program.clone(), additional_uniforms);

            let res = self.element.draw(frame, src, dst, damage);

            // Never forget to reset since its not our responsibility to manage texture shaders.
            BorrowMut::<GlesFrame>::borrow_mut(frame.glow_frame_mut()).clear_tex_program_override();

            res
        } else {
            self.element.draw(frame, src, dst, damage)
        }
    }

    fn underlying_storage(&self, renderer: &mut UdevRenderer<'a>) -> Option<UnderlyingStorage> {
        self.element.underlying_storage(renderer)
    }
}
