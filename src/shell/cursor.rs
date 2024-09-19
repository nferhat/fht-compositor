use std::cell::{Ref, RefCell};
use std::collections::hash_map::Entry;
use std::fmt::Debug;
use std::fs::File;
use std::io::Read;
use std::sync::LazyLock;
use std::time::Duration;

use rustc_hash::FxHashMap;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData};
use smithay::utils::{Logical, Physical, Point, Scale, Size, Transform};
use smithay::wayland::compositor::with_states;
use xcursor::parser::parse_xcursor;
use xcursor::CursorTheme;

use fht_compositor_config::Cursor;
use crate::renderer::FhtRenderer;

pub struct CursorThemeManager {
    // Image cache is keyed by icon type and cursor scale.
    // TODO: Fractional scaling when possible? (needs rewrite of MemoryRenderElement)
    cursor_image_cache: RefCell<FxHashMap<(i32, CursorIcon), Image>>,
    image_status: CursorImageStatus,
    cursor_theme: CursorTheme,
    config: Cursor,
}

impl CursorThemeManager {
    pub fn new(config: Cursor) -> Self {
        let cursor_theme = CursorTheme::load(&config.name);

        Self {
            cursor_image_cache: RefCell::new(FxHashMap::default()),
            image_status: CursorImageStatus::default_named(),
            cursor_theme,
            config,
        }
    }

    #[profiling::function]
    pub fn reload(&mut self, new_config: Cursor) {
        if self.config != new_config {
            self.config = new_config;
            self.cursor_theme = CursorTheme::load(&self.config.name);
            self.cursor_image_cache.borrow_mut().clear();
        }
    }

    pub fn image_status(&self) -> &CursorImageStatus {
        &self.image_status
    }

    pub fn set_image_status(&mut self, image_status: CursorImageStatus) {
        self.image_status = image_status
    }

    #[profiling::function]
    fn load_cursor_image<'a>(
        &'a self,
        cursor_icon: CursorIcon,
        cursor_scale: i32,
    ) -> Result<Ref<'a, Image>, Error> {
        if let Entry::Vacant(entry) = self
            .cursor_image_cache
            .borrow_mut()
            .entry((cursor_scale, cursor_icon))
        {
            let icon_path = self
                .cursor_theme
                .load_icon(cursor_icon.name())
                .or_else(|| {
                    for alt_name in cursor_icon.alt_names() {
                        if let Some(icon) = self.cursor_theme.load_icon(alt_name) {
                            return Some(icon);
                        }
                    }
                    None
                })
                .ok_or_else(|| Error::NoCursorIcon(cursor_icon))?;

            let mut cursor_file = File::open(icon_path)?;
            let mut cursor_file_data = Vec::new();
            let _ = cursor_file.read_to_end(&mut cursor_file_data)?;

            let images = parse_xcursor(&cursor_file_data).ok_or(Error::Parse)?;
            let size = self.config.size as i32 * cursor_scale;
            // pick the cursor image closest to the user desired size
            let (width, height) = images
                .iter()
                .min_by_key(|image| (size - image.size as i32).abs())
                .map(|image| (image.width, image.height))
                .unwrap();

            let mut animation_duration = Duration::ZERO;
            let mut frames = vec![];
            for image in images {
                if image.height != height || image.width != width {
                    continue;
                }

                let buffer = MemoryRenderBuffer::from_slice(
                    &image.pixels_rgba,
                    Fourcc::Argb8888,
                    Size::from((width as i32, height as i32)),
                    cursor_scale,
                    Transform::Normal,
                    None,
                );
                let hotspot = Point::from((image.xhot as i32, image.yhot as i32));
                let delay = Duration::from_millis(u64::from(image.delay));
                animation_duration += delay;
                frames.push(Frame {
                    buffer,
                    hotspot,
                    delay,
                });
            }

            entry.insert(Image {
                frames,
                animation_duration,
            });
        }

        Ok(Ref::map(self.cursor_image_cache.borrow(), |cache| {
            cache.get(&(cursor_scale, cursor_icon)).unwrap()
        }))
    }

    #[profiling::function]
    pub fn render<R>(
        &self,
        renderer: &mut R,
        mut location: Point<i32, Physical>,
        scale: Scale<f64>,
        cursor_scale: i32,
        alpha: f32,
        time: Duration,
    ) -> Result<Vec<CursorRenderElement<R>>, R::FhtError>
    where
        R: FhtRenderer,
    {
        match self.image_status {
            CursorImageStatus::Hidden => Ok(vec![]),
            CursorImageStatus::Surface(ref wl_surface) => {
                let hotspot = with_states(wl_surface, |states| {
                    states
                        .data_map
                        .get::<CursorImageSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .hotspot
                })
                .to_physical_precise_round(scale);
                location -= hotspot;

                Ok(
                    render_elements_from_surface_tree::<_, CursorRenderElement<R>>(
                        renderer,
                        wl_surface,
                        location,
                        scale,
                        alpha,
                        Kind::Cursor,
                    ),
                )
            }
            CursorImageStatus::Named(cursor_icon) => {
                let Ok(cursor_image) = self.load_cursor_image(cursor_icon, cursor_scale) else {
                    return self.render_with_fallback_cursor_data(renderer, location, alpha);
                };
                let frame = cursor_image.get_frame(time);
                location -= frame.hotspot.to_physical_precise_round(scale);
                Ok(vec![CursorRenderElement::Memory(
                    MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        location.to_f64(),
                        &frame.buffer,
                        Some(alpha),
                        None,
                        None,
                        Kind::Cursor,
                    )?,
                )])
            }
        }
    }

    fn render_with_fallback_cursor_data<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        alpha: f32,
    ) -> Result<Vec<CursorRenderElement<R>>, R::FhtError> {
        static RENDER_BUFFER: LazyLock<MemoryRenderBuffer> = LazyLock::new(|| {
            MemoryRenderBuffer::from_slice(
                include_bytes!("../../res/cursor.rgba"),
                Fourcc::Argb8888,
                Size::from((64, 64)),
                1,
                Transform::Normal,
                None,
            )
        });
        Ok(vec![CursorRenderElement::Memory(
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                location.to_f64(),
                &RENDER_BUFFER,
                Some(alpha),
                None,
                None,
                Kind::Cursor,
            )?,
        )])
    }
}

struct Image {
    frames: Vec<Frame>,
    animation_duration: Duration,
}

impl Image {
    #[profiling::function]
    fn get_frame(&self, now: Duration) -> &Frame {
        if self.animation_duration.is_zero() {
            return &self.frames[0];
        }

        let mut now = now.as_millis() % self.animation_duration.as_millis();
        for frame in &self.frames {
            let delay = frame.delay.as_millis();
            if now < delay {
                return frame;
            }
            now -= delay;
        }

        unreachable!("Cursor theme has no images for frame")
    }
}

struct Frame {
    buffer: MemoryRenderBuffer,
    hotspot: Point<i32, Logical>,
    delay: Duration,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Cursor theme does not have cursor icon: {0}")]
    NoCursorIcon(CursorIcon),
    #[error("Failed to open cursor image file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse cursor image file")]
    Parse,
}

crate::fht_render_elements! {
    CursorRenderElement<R> => {
        Surface = WaylandSurfaceRenderElement<R>,
        Memory = MemoryRenderBufferRenderElement<R>,
    }
}
