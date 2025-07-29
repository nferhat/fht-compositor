//! Shaadow rendering for tiles.

use std::cell::RefCell;

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::Uniform;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::renderer::shaders::{ShaderElement, Shaders};
use crate::renderer::FhtRenderer;

/// A box-shadow around a tile.
#[derive(Clone, Debug)]
pub struct Shadow {
    element: RefCell<Option<ShaderElement>>,
    geometry: Rectangle<i32, Logical>,
    parameters: Parameters,
}

/// [`Shadow`] parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parameters {
    /// Whether the shadow is disabled.
    pub disable: bool,
    /// Whether we should draw the shadow for floating windows only.
    pub floating_only: bool,
    /// The shadow color.
    pub color: [f32; 4],
    /// The blur sigma/radius of the shadow.
    pub blur_sigma: f32,
    /// The corner radius of the shadowed rectangle.
    pub corner_radius: f32,
}

impl Shadow {
    pub fn new(geometry: Rectangle<i32, Logical>, parameters: Parameters) -> Self {
        Self {
            element: RefCell::new(None), // we initialize the element on the first render call
            geometry,
            parameters,
        }
    }

    /// Resize this border.
    pub fn set_geometry(&mut self, geometry: Rectangle<i32, Logical>) {
        if self.geometry == geometry {
            return;
        }

        self.geometry = geometry;
        if let Some(element) = self.element.get_mut() {
            let mut expanded_geo = geometry;
            let Parameters { blur_sigma, .. } = self.parameters;
            expanded_geo.loc -= Point::from((blur_sigma as i32, blur_sigma as i32));
            expanded_geo.size += Size::from((blur_sigma as i32, blur_sigma as i32)).upscale(2);
            element.resize(expanded_geo, None);
        }
    }

    /// Update the border parameters
    pub fn update_parameters(&mut self, parameters: Parameters) {
        if self.parameters == parameters {
            return;
        }

        self.parameters = parameters;
        let uniforms = self.uniforms();
        if let Some(element) = self.element.get_mut() {
            element.update_uniforms(uniforms);
        }
    }

    /// Get the last parameters set by [`Self::udpate_parameters`]
    pub fn parameters(&self) -> &Parameters {
        &self.parameters
    }

    fn uniforms(&self) -> Vec<Uniform<'static>> {
        let Parameters {
            color,
            blur_sigma,
            corner_radius,
            ..
        } = self.parameters;

        vec![
            Uniform::new("shadow_color", color),
            Uniform::new("blur_sigma", blur_sigma),
            Uniform::new("corner_radius", corner_radius),
        ]
    }

    /// Get a [`BorderRenderElement`] from this Border
    ///
    /// If returns `None`, the border should not be rendered.
    pub fn render(
        &self,
        renderer: &mut impl FhtRenderer,
        alpha: f32,
        is_floating: bool,
    ) -> Option<ShaderElement> {
        if self.parameters.disable
            || self.parameters.color[3] == 0.0
            || (!is_floating && self.parameters.floating_only)
        {
            return None;
        }

        let mut guard = self.element.borrow_mut();
        let element = guard.get_or_insert_with(|| {
            let program = Shaders::get(renderer.glow_renderer()).box_shadow.clone();
            let mut expanded_geo = self.geometry;
            let Parameters { blur_sigma, .. } = self.parameters;
            expanded_geo.loc -= Point::from((blur_sigma as i32, blur_sigma as i32));
            expanded_geo.size += Size::from((blur_sigma as i32, blur_sigma as i32)).upscale(2);

            ShaderElement::new(
                program,
                expanded_geo,
                None,
                1.0,
                self.uniforms(),
                Kind::Unspecified,
            )
        });
        element.set_alpha(alpha);

        Some(element.clone())
    }
}
