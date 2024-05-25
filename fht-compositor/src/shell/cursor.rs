use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Debug;
use std::io::Read;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::input::pointer::{CursorIcon, CursorImageAttributes, CursorImageStatus};
use smithay::utils::{Physical, Point, Scale, Transform};
use smithay::wayland::compositor;
use xcursor::parser::{parse_xcursor, Image};
use xcursor::CursorTheme;

use crate::config::{CursorConfig, CONFIG};
use crate::renderer::texture_element::FhtTextureElement;
use crate::renderer::FhtRenderer;

static FALLBACK_CURSOR_DATA: &[u8] = include_bytes!("../../res/cursor.rgba");

fn get_fallback_cursor_data(_: impl std::error::Error) -> Rc<CursorImage> {
    Rc::new(CursorImage {
        frames: vec![Image {
            size: 32,
            width: 64,
            height: 64,
            xhot: 1,
            yhot: 1,
            delay: 0,
            pixels_rgba: Vec::from(FALLBACK_CURSOR_DATA),
            pixels_argb: vec![],
        }],
        animation_duration: 0,
    })
}

pub type CursorImageCache = HashMap<(CursorIcon, i32), Rc<CursorImage>>;
pub type CursorTextureCache = HashMap<(CursorIcon, i32), Vec<(Image, Box<dyn Any>)>>;

/// A cursor theme manager.
///
/// This will manage the active cursor theme expressed by the [`FhtConfig`], and cache the images
/// to render them later.
pub struct CursorThemeManager {
    /// A cache of the different cursor images associated with their cursor icons and scale.
    image_cache: RefCell<CursorImageCache>,

    /// A cache of the different rendered cursor texture associated with their cursor icons.
    texture_cache: RefCell<CursorTextureCache>,

    /// The last known cursor image status, to know what to render.
    pub image_status: Arc<Mutex<CursorImageStatus>>,

    /// The current cursor theme, from which [`CursorImage`]s are generated.
    cursor_theme: CursorTheme,
    // Additional info of the cursor theme, so we don't spam the config
    cursor_theme_name: String,
    cursor_theme_size: u32,
}

impl CursorThemeManager {
    /// Initialize the cursor theme manager.
    pub fn new() -> Self {
        let CursorConfig { name, size } = CONFIG.general.cursor.clone();
        let image_status = CursorImageStatus::default_named();
        let cursor_theme = CursorTheme::load(&name);

        std::env::set_var("XCURSOR_THEME", &name);
        std::env::set_var("XCURSOR_SIZE", size.to_string());

        Self {
            image_cache: RefCell::new(HashMap::new()),
            texture_cache: RefCell::new(HashMap::new()),

            image_status: Arc::new(Mutex::new(image_status)),

            cursor_theme,
            cursor_theme_name: name,
            cursor_theme_size: size,
        }
    }

    /// Reload the cursor theme manager configuration.
    ///
    /// This is only effective if the name or size have changed.
    #[profiling::function]
    pub fn reload(&mut self) {
        let CursorConfig { name, size } = CONFIG.general.cursor.clone();
        if self.cursor_theme_name == name && self.cursor_theme_size == size {
            return;
        }

        std::env::set_var("XCURSOR_THEME", &name);
        std::env::set_var("XCURSOR_SIZE", size.to_string());

        self.image_cache.borrow_mut().clear();
        self.texture_cache.borrow_mut().clear();
        if self.cursor_theme_name == name && self.cursor_theme_size != size {
            return;
        }

        let new_cursor_theme = CursorTheme::load(&name);
        self.cursor_theme = new_cursor_theme;

        self.cursor_theme_size = size;
        self.cursor_theme_name = name;
    }

    /// Loads the cursor image associated with this [`CursorIcon`].
    ///
    /// Returns Ok if the image was already loaded/got successfully loaded, returns Err if loading
    /// failed.
    #[profiling::function]
    fn load_cursor_image(
        &self,
        cursor_icon: CursorIcon,
        cursor_scale: i32,
    ) -> Result<Rc<CursorImage>, Error> {
        let mut image_cache = self.image_cache.borrow_mut();
        if let Some(image) = image_cache.get(&(cursor_icon, cursor_scale)) {
            return Ok(image.clone());
        }

        // Load images, using old names as a fallback
        let mut maybe_icon_path = self
            .cursor_theme
            .load_icon(cursor_icon.name())
            .ok_or(Error::NoCursorIcon);
        for alt_name in cursor_icon.alt_names() {
            maybe_icon_path = self
                .cursor_theme
                .load_icon(alt_name)
                .ok_or(Error::NoCursorIcon);
            if maybe_icon_path.is_ok() {
                break;
            }
        }

        // Load data
        let icon_path = maybe_icon_path?;
        let mut cursor_file = std::fs::File::open(icon_path).map_err(Error::File)?;
        let mut cursor_data = Vec::new();
        cursor_file
            .read_to_end(&mut cursor_data)
            .map_err(Error::File)?;

        // Filter by size
        // Follow the nominal size of the cursor to choose the closest ones
        //
        // Doing this here will avoid us checking for nearest images on each render
        let size = self.cursor_theme_size as i32 * cursor_scale;
        let mut images = parse_xcursor(&cursor_data).ok_or(Error::Parse)?;
        let (width, height) = images
            .iter()
            .min_by_key(|image| (size - image.size as i32).abs())
            .map(|image| (image.width, image.height))
            .unwrap();
        images.retain(move |image| image.width == width && image.height == height);

        let animation_duration = images.iter().fold(0, |acc, image| acc + image.delay);
        let cursor_image = Rc::new(CursorImage {
            frames: images,
            animation_duration,
        });
        image_cache.insert((cursor_icon, cursor_scale), cursor_image.clone());

        Ok(cursor_image)
    }

    /// Render the cursor based on the current [`CursorImageStatus`] stored here.
    #[profiling::function]
    pub fn render_cursor<R, E>(
        &self,
        renderer: &mut R,
        mut location: Point<i32, Physical>,
        scale: Scale<f64>,
        cursor_scale: i32,
        alpha: f32,
        time: Duration,
    ) -> Vec<E>
    where
        R: FhtRenderer,
        E: From<CursorRenderElement<R>>,
    {
        let image_status = &*self.image_status.lock().unwrap();
        match *image_status {
            CursorImageStatus::Hidden => vec![],
            CursorImageStatus::Surface(ref wl_surface) => {
                let hotspot = compositor::with_states(wl_surface, |states| {
                    states
                        .data_map
                        .get::<Mutex<CursorImageAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .hotspot
                })
                .to_physical_precise_round(scale);
                location -= hotspot;

                render_elements_from_surface_tree::<_, CursorRenderElement<R>>(
                    renderer,
                    wl_surface,
                    location,
                    scale,
                    alpha,
                    Kind::Cursor,
                )
                .into_iter()
                .map(E::from)
                .collect()
            }
            CursorImageStatus::Named(cursor_icon) => {
                let cursor_image = self
                    .load_cursor_image(cursor_icon, cursor_scale)
                    .unwrap_or_else(get_fallback_cursor_data);
                let (frame, hotspot) = cursor_image.frame(time.as_millis() as u32);
                location -= hotspot;

                // Get the cursor texture, and generate them all if not already present
                let mut texture_cache = self.texture_cache.borrow_mut();
                let frame_texture_cache = texture_cache
                    .entry((cursor_icon, cursor_scale))
                    .or_default();

                let maybe_frame_texture = frame_texture_cache
                    .iter()
                    .find(|(f, _)| f == frame)
                    .and_then(|(_, t)| t.downcast_ref::<TextureBuffer<R::TextureId>>());
                let frame_texture = match maybe_frame_texture {
                    Some(t) => t,
                    None => {
                        let texture = TextureBuffer::from_memory(
                            renderer,
                            &frame.pixels_rgba,
                            Fourcc::Abgr8888,
                            (frame.width as i32, frame.height as i32),
                            false,
                            cursor_scale as i32,
                            Transform::Normal,
                            None,
                        )
                        .expect("Failed to import cursor bitmap");
                        frame_texture_cache.push((frame.clone(), Box::new(texture.clone())));
                        frame_texture_cache
                            .last()
                            .and_then(|(_, i)| i.downcast_ref::<TextureBuffer<R::TextureId>>())
                            .unwrap()
                    }
                };

                vec![E::from(CursorRenderElement::Texture(FhtTextureElement(
                    TextureRenderElement::from_texture_buffer(
                        location.to_f64(),
                        &frame_texture,
                        None,
                        None,
                        None,
                        Kind::Cursor,
                    ),
                )))]
            }
        }
    }
}

pub struct CursorImage {
    /// All the frames composing this cursor icon.
    frames: Vec<Image>,

    /// The duration on which all the frames loop at once, in milliseconds.
    ///
    /// Cache it here to avoid recounting them each time.
    animation_duration: u32,
}

impl CursorImage {
    /// Given at a time, which frame to show.
    ///
    /// This function warps time, do a frame shown at 5ms will also be shown at 105ms.
    pub fn frame(&self, mut millis: u32) -> (&Image, Point<i32, Physical>) {
        if self.animation_duration == 0 {
            let frame = &self.frames[0];
            return (frame, (frame.xhot as i32, frame.yhot as i32).into());
        }

        millis %= self.animation_duration;
        for frame in &self.frames {
            if millis < frame.delay {
                return (frame, (frame.xhot as i32, frame.yhot as i32).into());
            }
            millis -= frame.delay;
        }

        unreachable!("Added cursor theme has no images for frame")
    }
}

#[derive(Debug)]
enum Error {
    NoCursorIcon,
    File(std::io::Error),
    Parse,
}

impl std::error::Error for Error {}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCursorIcon => f.write_str("Cursor theme has no default cursor image"),
            Self::File(inner) => std::fmt::Display::fmt(inner, f),
            Self::Parse => f.write_str("Failed to parse cursor theme file"),
        }
    }
}

crate::fht_render_elements! {
    CursorRenderElement<R> => {
        Surface = WaylandSurfaceRenderElement<R>,
        Texture = FhtTextureElement<R::TextureId>,
    }
}
