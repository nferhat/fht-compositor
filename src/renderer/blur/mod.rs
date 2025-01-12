//! Blurring algorithm and system integrated into smithay.
//!
//! It is not perfect at the moment but currently I am satisfied enough with how it looks. The
//! actual underlying algorithm is Dual-Kawase, with downscaling then upscaling steps.
//!
//! - <https://github.com/alex47/Dual-Kawase-Blur>
//! - <https://github.com/wlrfx/scenefx>
//! - <https://www.shadertoy.com/view/3td3W8>

pub mod element;

use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::element::TextureShaderElement;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexProgram, GlesTexture, Uniform};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::{Bind, Blit, Renderer, Texture, TextureFilter, Unbind};
use smithay::output::Output;
use smithay::reexports::gbm::Format;
use smithay::utils::{Logical, Rectangle, Size, Transform};
use smithay::wayland::shell::wlr_layer::Layer;

use super::shaders::Shaders;
use super::{layer_elements, render_elements, FhtRenderer};
use crate::output::OutputExt;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum CurrentBuffer {
    /// We are currently sampling from normal buffer, and rendering in the swapped/alternative.
    #[default]
    Normal,
    /// We are currently sampling from swapped buffer, and rendering in the normal.
    Swapped,
}

impl CurrentBuffer {
    pub fn swap(&mut self) {
        *self = match self {
            // sampled from normal, render to swapped
            Self::Normal => Self::Swapped,
            // sampled fro swapped, render to normal next
            Self::Swapped => Self::Normal,
        }
    }
}

/// Effect framebuffers associated with each output.
pub struct EffectsFramebuffers {
    /// Contains the main buffer blurred contents
    pub optimized_blur: GlesTexture,
    /// Whether the optimizer blur buffer is dirty
    pub optimized_blur_dirty: bool,
    // /// Contains the original pixels before blurring to draw with in case of artifacts.
    // blur_saved_pixels: GlesTexture,
    // The blur algorithms (dual-kawase) swaps between these two whenever scaling the image
    effects: GlesTexture,
    effects_swapped: GlesTexture,
    /// The buffer we are currently rendering/sampling from.
    ///
    /// In order todo the up/downscaling, we render into different buffers. On each pass, we render
    /// into a different buffer with downscaling/upscaling (depending on which pass we are at)
    ///
    /// One exception is that if we are on the first pass, we are on [`CurrentBuffer::Initial`], we
    /// are sampling from [`Self::blit_buffer`] from initial screen contents.
    current_buffer: CurrentBuffer,
}

type EffectsFramebufffersUserData = Rc<RefCell<EffectsFramebuffers>>;

impl EffectsFramebuffers {
    /// Get the assiciated [`EffectsFramebuffers`] with this output.
    pub fn get(output: &Output) -> RefMut<'_, Self> {
        let user_data = output
            .user_data()
            .get::<EffectsFramebufffersUserData>()
            .unwrap();
        RefCell::borrow_mut(user_data)
    }

    /// Initialize the [`EffectsFramebuffers`] for an [`Output`].
    ///
    /// The framebuffers handles live inside the Output's user data, use [`Self::get`] to access
    /// them.
    pub fn init_for_output(output: &Output, renderer: &mut impl FhtRenderer) {
        let output_size = output.geometry().size;

        fn create_buffer(renderer: &mut impl FhtRenderer, size: Size<i32, Logical>) -> GlesTexture {
            renderer
                .create_buffer(Format::Abgr8888, size.to_buffer(1, Transform::Normal))
                .expect("gl should always be able to create buffers")
        }

        let this = EffectsFramebuffers {
            optimized_blur: renderer
                .create_buffer(
                    Format::Abgr8888,
                    output_size.to_buffer(1, Transform::Normal),
                )
                .unwrap(),
            optimized_blur_dirty: true,
            // blur_saved_pixels: renderer
            //     .create_buffer(
            //         Format::Abgr8888,
            //         output_size.to_buffer(1, Transform::Normal),
            //     )
            //     .unwrap(),
            effects: create_buffer(renderer, output_size),
            effects_swapped: create_buffer(renderer, output_size),
            current_buffer: CurrentBuffer::Normal,
        };

        let user_data = output.user_data();
        assert!(
            user_data.insert_if_missing(|| Rc::new(RefCell::new(this))),
            "EffectsFrambuffers::init_for_output should only be called once!"
        );
    }

    /// Render the optimized blur buffer again
    pub fn update_optimized_blur_buffer(
        &mut self,
        renderer: &mut GlowRenderer,
        output: &Output,
        scale: i32,
    ) {
        // first render layer shell elements
        let elements = layer_elements(renderer, output, Layer::Background)
            .into_iter()
            .chain(layer_elements(renderer, output, Layer::Bottom));
        renderer.bind(self.effects.clone()).unwrap();
        let output_rect = output.geometry().to_physical(scale);
        let _ = render_elements(
            renderer,
            output_rect.size,
            scale as f64,
            Transform::Normal,
            elements,
        )
        .expect("failed to render for optimized blur buffer");
        self.current_buffer = CurrentBuffer::Normal;

        let shaders = Shaders::get(renderer);
        let blur_down = shaders.blur_down.clone();
        let blur_up = shaders.blur_up.clone();

        // NOTE: If we only do one pass its kinda ugly, there must be at least
        // n=2 passes in order to have good sampling
        let half_pixel = [
            0.5 / (output_rect.size.w as f32 / 2.0),
            0.5 / (output_rect.size.h as f32 / 2.0),
        ];
        for _ in 0..N_PASSES {
            render_blur_pass(renderer, self, blur_down.clone(), half_pixel);
        }

        let half_pixel = [
            0.5 / (output_rect.size.w as f32 * 2.0),
            0.5 / (output_rect.size.h as f32 * 2.0),
        ];
        for _ in 0..N_PASSES {
            render_blur_pass(renderer, self, blur_up.clone(), half_pixel);
        }

        // Now blit from the last render buffer into optimized_blur
        // We are already bound so its just a blit
        renderer
            .blit_to(
                self.optimized_blur.clone(),
                Rectangle::from_size(output_rect.size),
                Rectangle::from_size(output_rect.size),
                TextureFilter::Linear,
            )
            .unwrap();
    }

    /// Get the buffer that was sampled from in the previous pass.
    pub fn sample_buffer(&self) -> GlesTexture {
        match self.current_buffer {
            CurrentBuffer::Normal => self.effects.clone(),
            CurrentBuffer::Swapped => self.effects_swapped.clone(),
        }
    }

    /// Get the buffer that was rendered into in the previous pass.
    pub fn render_buffer(&self) -> GlesTexture {
        match self.current_buffer {
            CurrentBuffer::Normal => self.effects_swapped.clone(),
            CurrentBuffer::Swapped => self.effects.clone(),
        }
    }
}

/// Render blur pass.
///
/// When we want to get the main buffer blur, we have to go multiple passes in order to get
/// something that looks good, this is up to the user to configure.
fn render_blur_pass(
    renderer: &mut GlowRenderer,
    effects_framebuffers: &mut EffectsFramebuffers,
    blur_program: GlesTexProgram,
    half_pixel: [f32; 2],
) {
    // Swap buffers and bind
    //
    // NOTE: Since we are just swapping between two buffers of the same size, we must make sure that
    // the shader code accounts for this! I'd rather not keep multiple buffers alive for different
    // passes.
    let sample_buffer = effects_framebuffers.sample_buffer();

    // We use a texture render element with a custom GlesTexProgram in order todo the blurring
    // At least this is what swayfx/scenefx do, but they just use gl calls directly.
    let size = sample_buffer.size().to_logical(1, Transform::Normal);
    let texture = TextureRenderElement::from_static_texture(
        Id::new(),
        renderer.id(),
        (0., 0.),
        sample_buffer,
        1,
        Transform::Normal,
        None,
        None,
        None, // NOTE: the texture size is the same as output_rect
        None,
        Kind::Unspecified,
    );
    let texture = TextureShaderElement::new(
        texture,
        blur_program,
        vec![
            Uniform::new("radius", BLUR_RADIUS as f32),
            Uniform::new("half_pixel", half_pixel),
        ],
    );

    // NOTE: I think the binding/unbinding should always work since if that fails the EGL context
    // is not current and in this case its just game over for the render state.
    //
    // I should probably confirm this
    let target_buffer = effects_framebuffers.render_buffer();
    renderer.bind(target_buffer).expect("gl should bind");
    let _ = render_elements(
        renderer,
        size.to_physical_precise_round(1),
        1.0,
        Transform::Normal,
        ([&texture]).iter(),
    )
    .unwrap();

    effects_framebuffers.current_buffer.swap();
}

const N_PASSES: u32 = 2;
const BLUR_RADIUS: i32 = 5;
