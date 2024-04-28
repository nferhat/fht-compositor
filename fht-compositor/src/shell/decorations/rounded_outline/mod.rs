use std::borrow::{Borrow, BorrowMut};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use smithay::backend::renderer::element::{Element, Kind};
use smithay::backend::renderer::gles::element::PixelShaderElement;
use smithay::backend::renderer::gles::{
    GlesPixelProgram, GlesRenderer, Uniform, UniformName, UniformType,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Rectangle;

use crate::backend::render::AsGlowRenderer;
use crate::config::ColorConfig;
use crate::utils::geometry::{Local, RectLocalExt};

pub type RoundedOutlineShaderCache =
    HashMap<WlSurface, (RoundedOutlineShaderSettings, PixelShaderElement)>;

#[derive(Debug, Clone, Copy, PartialEq)]
/// Settings to control a rounded outline shader element
pub struct RoundedOutlineShaderSettings {
    /// The thickness to use.
    pub thickness: u8,
    /// The radius.
    pub radius: f32,
    /// The color, either a solid one or a gradient.
    pub color: ColorConfig,
}

pub struct RoundedOutlineShader {
    pub program: GlesPixelProgram,
    pub element_cache: Rc<RefCell<RoundedOutlineShaderCache>>,
}

impl RoundedOutlineShader {
    const SRC: &'static str = include_str!("./shader.frag");

    /// Initialize the shader for the given renderer.
    ///
    /// The shader is stored inside the renderer's EGLContext user data.
    pub fn init(renderer: &mut impl AsGlowRenderer) {
        let renderer = renderer.glow_renderer_mut();
        let program = {
            let gles_renderer: &mut GlesRenderer = renderer.borrow_mut();
            gles_renderer
                .compile_custom_pixel_shader(
                    Self::SRC,
                    &[
                        UniformName::new("v_start_color", UniformType::_4f),
                        UniformName::new("v_end_color", UniformType::_4f),
                        UniformName::new("v_gradient_direction", UniformType::_2f),
                        UniformName::new("radius", UniformType::_1f),
                        UniformName::new("half_thickness", UniformType::_1f),
                    ],
                )
                .expect("Failed to compile rounded outline shader!")
        };
        renderer
            .egl_context()
            .user_data()
            .insert_if_missing(|| RoundedOutlineShader {
                program,
                element_cache: Rc::new(RefCell::new(RoundedOutlineShaderCache::new())),
            });
    }

    /// Get a reference to the shader instance stored in this renderer EGLContext userdata.
    ///
    /// If you didn't initialize the shader before, this function will do it for you.
    pub fn get(renderer: &mut impl AsGlowRenderer) -> &Self {
        Borrow::<GlesRenderer>::borrow(renderer.glow_renderer())
            .egl_context()
            .user_data()
            .get::<RoundedOutlineShader>()
            .expect("Shaders didn't initialize!")
    }

    /// Create a rounded outline element.
    ///
    /// The geo argument should be local to the output where the wl_surface is being drawn.
    pub fn element(
        renderer: &mut impl AsGlowRenderer,
        scale: f64,
        alpha: f32,
        wl_surface: &WlSurface,
        mut geo: Rectangle<i32, Local>,
        settings: RoundedOutlineShaderSettings,
    ) -> PixelShaderElement {
        // Scaled thickness only matters to make the border thickness in the shader.
        // Geometry shouldd still obey the normal thickness
        let thickness = settings.thickness as i32;
        let scaled_thickness = settings.thickness as f32 * scale as f32;
        geo.loc -= (thickness, thickness).into();
        geo.size += (2 * thickness, 2 * thickness).into();

        let shader = Self::get(renderer);
        let mut element_cache = RefCell::borrow_mut(&shader.element_cache);

        if let Some((_, element)) = element_cache
            .get_mut(wl_surface)
            .filter(|(old_settings, _)| &settings == old_settings)
        {
            if element.geometry(1.0.into()).to_logical(1) != geo.as_logical() {
                element.resize(geo.as_logical(), None);
            }
            return element.clone();
        }

        let (start_color, end_color, angle) = match settings.color {
            ColorConfig::Solid(color) => (color, color, 0.0),
            ColorConfig::Gradient { start, end, angle } => {
                (start, end, angle * std::f32::consts::PI)
            }
        };
        let gradient_direction = [angle.cos(), angle.sin()];

        let mut element = PixelShaderElement::new(
            shader.program.clone(),
            geo.as_logical(),
            None, //TODO
            alpha,
            vec![
                Uniform::new("v_start_color", start_color),
                Uniform::new("v_end_color", end_color),
                Uniform::new("v_gradient_direction", gradient_direction),
                Uniform::new("half_thickness", scaled_thickness as f32 / 2f32),
                Uniform::new("radius", settings.radius),
            ],
            Kind::Unspecified,
        );

        if element.geometry(1.0.into()).to_logical(1) != geo.as_logical() {
            element.resize(geo.as_logical(), None);
        }

        element_cache.insert(wl_surface.clone(), (settings, element.clone()));
        element
    }
}
