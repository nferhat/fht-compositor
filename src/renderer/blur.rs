//! Blurring algorithm and system integrated into smithay.
//!
//! It is not perfect at the moment but currently I am satisfied enough with how it looks. The
//! actual underlying algorithm is Dual-Kawase, with downscaling then upscaling steps.
//!
//! - <https://github.com/alex47/Dual-Kawase-Blur>
//! - <https://github.com/wlrfx/scenefx>
//! - <https://www.shadertoy.com/view/3td3W8>

use std::borrow::BorrowMut;
use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::element::TextureShaderElement;
use smithay::backend::renderer::gles::{
    GlesRenderer, GlesTarget, GlesTexProgram, GlesTexture, Uniform,
};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::{Bind, Blit, Renderer, Texture, TextureFilter, Unbind};
use smithay::output::Output;
use smithay::reexports::gbm::Format;
use smithay::utils::{Logical, Size, Transform};

use super::shaders::Shaders;
use super::{render_elements, FhtRenderer};
use crate::output::OutputExt;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum CurrentBuffer {
    /// We are currently sampling from normal buffer, and rendering in the swapped/alternative.
    #[default]
    Normal,
    /// We are currently sampling from swapped buffer, and rendering in the normal.
    Swapped,
    /// We are currently rendering from the initial blitted buffer, and rendering in the normal
    /// buffer
    Initial,
}

impl CurrentBuffer {
    pub fn swap(&mut self) {
        *self = match self {
            // sampled from normal, render to swapped
            Self::Normal => Self::Swapped,
            // sampled fro swapped, render to normal next
            Self::Swapped => Self::Normal,
            // sampled from blit, render into normal next
            Self::Initial => Self::Normal,
        }
    }
}

/// Effect framebuffers associated with each output.
pub struct EffectsFramebuffers {
    // /// Contains the main buffer blurred contents
    // optimized_blur: GlesTexture,
    // /// Contains the original pixels before blurring to draw with in case of artifacts.
    // blur_saved_pixels: GlesTexture,
    blit_buffer: GlesTexture,
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
            // optimized_blur: renderer
            //     .create_buffer(
            //         Format::Abgr8888,
            //         output_size.to_buffer(1, Transform::Normal),
            //     )
            //     .unwrap(),
            // blur_saved_pixels: renderer
            //     .create_buffer(
            //         Format::Abgr8888,
            //         output_size.to_buffer(1, Transform::Normal),
            //     )
            //     .unwrap(),
            blit_buffer: create_buffer(renderer, output_size),
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

    /// Get the buffer that was sampled from in the previous pass.
    pub fn sample_buffer(&self) -> GlesTexture {
        match self.current_buffer {
            CurrentBuffer::Normal => self.effects.clone(),
            CurrentBuffer::Swapped => self.effects_swapped.clone(),
            CurrentBuffer::Initial => self.blit_buffer.clone(),
        }
    }

    /// Get the buffer that was rendered into in the previous pass.
    pub fn render_buffer(&self) -> GlesTexture {
        match self.current_buffer {
            CurrentBuffer::Normal => self.effects_swapped.clone(),
            CurrentBuffer::Swapped => self.effects.clone(),
            CurrentBuffer::Initial => self.effects.clone(),
        }
    }
}

/// Render blur pass.
///
/// When we want to get the main buffer blur, we have to go multiple passes in order to get
/// something that looks good, this is up to the user to configure.
fn render_blur_pass<'frame>(
    renderer: &mut GlowRenderer,
    effects_framebuffers: &mut EffectsFramebuffers,
    blur_program: GlesTexProgram,
    size: Size<i32, Logical>,
    half_pixel: [f32; 2],
) {
    // Swap buffers and bind
    //
    // NOTE: Since we are just swapping between two buffers of the same size, we must make sure that
    // the shader code accounts for this! I'd rather not keep multiple buffers alive for different
    // passes.
    dbg!(&effects_framebuffers.current_buffer);
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
    renderer.unbind().expect("gl should unbind");

    effects_framebuffers.current_buffer.swap();
    dbg!(&effects_framebuffers.current_buffer);
}

// fn blur_settings_to_size(passes: u32, radius: i32) -> i32 {
//     return 2i32.pow(passes + 1) * radius;
// }

const N_PASSES: u32 = 1;
const BLUR_RADIUS: i32 = 3;

fn get_main_buffer_blur(renderer: &mut GlowRenderer, output: &Output) -> GlesTexture {
    let output_rect = output.geometry();

    let effects_framebuffers = &mut *EffectsFramebuffers::get(output);
    let shaders = Shaders::get(renderer);
    let blur_down = shaders.blur_down.clone();
    let blur_up = shaders.blur_up.clone();

    // Blit the current fb into our initial starting texture
    renderer
        .blit_to(
            effects_framebuffers.blit_buffer.clone(),
            output_rect.to_physical_precise_round(1),
            output_rect.to_physical_precise_round(1),
            TextureFilter::Linear,
        )
        .unwrap();
    effects_framebuffers.current_buffer = CurrentBuffer::Initial;

    let gles_renderer: &mut GlesRenderer = renderer.borrow_mut();
    let previous_target = gles_renderer.target_mut().take();

    // NOTE: If we only do one pass its kinda ugly, there must be at least
    // n=2 passes in order to have good sampling
    let half_pixel = [
        0.5 / (output_rect.size.w as f32 / 2.0),
        0.5 / (output_rect.size.h as f32 / 2.0),
    ];
    for _ in 0..(N_PASSES + 1) {
        render_blur_pass(
            renderer,
            effects_framebuffers,
            blur_down.clone(),
            output_rect.size,
            half_pixel,
        );
    }

    let half_pixel = [
        0.5 / (output_rect.size.w as f32 * 2.0),
        0.5 / (output_rect.size.h as f32 * 2.0),
    ];
    for _ in 0..(N_PASSES + 1) {
        render_blur_pass(
            renderer,
            effects_framebuffers,
            blur_up.clone(),
            output_rect.size,
            half_pixel,
        );
    }

    match previous_target {
        Some(ref target) => match target {
            // NOTE: The drop impl of GlesTarget will automatically send destruction events
            GlesTarget::Image { dmabuf, .. } => renderer.bind(dmabuf.clone()).unwrap(),
            GlesTarget::Surface { surface } => renderer.bind(surface.clone()).unwrap(),
            GlesTarget::Texture { texture, .. } => renderer.bind(texture.clone()).unwrap(),
            GlesTarget::Renderbuffer { buf, .. } => renderer.bind(buf.clone()).unwrap(),
        },
        None => (), // we werent even bound yet!
    }

    effects_framebuffers.render_buffer()
}
