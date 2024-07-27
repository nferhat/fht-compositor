//! Egui implementation for the compositor.
//!
//! A lot of thd code ideas and impls are from
//! [smithay-egui](https://github.com/smithay/smithay-egui), with additional tailoring to fit the
//! compositor's needs.

use std::borrow::BorrowMut;
use std::sync::{Arc, Mutex};

use smithay::backend::allocator::Fourcc;
use smithay::backend::input::MouseButton;
use smithay::backend::renderer::element::texture::{TextureRenderBuffer, TextureRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::{self, GlesError, GlesTexture};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::{Bind, Frame, Offscreen, Renderer, Unbind};
use smithay::input::keyboard::{xkb, ModifiersState};
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Size, Transform};

use crate::renderer::texture_element::FhtTextureElement;

/// A single Egui element.
///
/// This element holds a single egui window in which egui's [`Context`] will draw,
pub struct EguiElement {
    size: Size<i32, Logical>,
    inner: Arc<Mutex<EguiElementInner>>,
}

impl EguiElement {
    /// Create a new [`EguiElement`] with a given `size`
    pub fn new(size: Size<i32, Logical>) -> Self {
        let xkb_keymap = xkb::Keymap::new_from_names(
            &xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            "",
            "",
            "",
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .unwrap();
        let xkb_state = xkb::State::new(&xkb_keymap);
        Self {
            size,
            inner: Arc::new(Mutex::new(EguiElementInner {
                context: egui::Context::default(),
                painter: None,
                last_pointer_position: None,
                last_modifiers: ModifiersState::default(),
                xkb_keymap,
                xkb_state,
                events: Vec::new(),
            })),
        }
    }

    /// Resize the element.
    pub fn set_size(&mut self, new_size: Size<i32, Logical>) {
        self.size = new_size;
        let mut guard = self.inner.lock().unwrap();
        if let Some(painter) = guard.painter.as_mut() {
            // New size, new buffer
            let _ = painter.render_buffer.take();
        }
    }

    /// Run the element's context, sending all the queued up events to the context.
    ///
    /// - `ui` is your function used to render the context.
    /// - `time` is the current system monotonic time.
    pub fn run(&self, ui: impl FnOnce(&egui::Context), time: std::time::Duration, scale: i32) {
        let mut guard = self.inner.lock().unwrap();
        let size = self.size.to_physical(scale);

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect {
                min: egui::pos2(0.0, 0.0),
                max: egui::pos2(size.w as f32, size.h as f32),
            }),
            time: Some(time.as_secs_f64()),
            predicted_dt: 1.0 / 60.0,
            modifiers: convert_modifiers(guard.last_modifiers),
            events: guard.events.drain(..).collect(),
            hovered_files: Vec::with_capacity(0),
            dropped_files: Vec::with_capacity(0),
            focused: true,
            max_texture_side: guard
                .painter
                .as_ref()
                .map(|painter| painter.max_texture_size),
            ..Default::default()
        };

        let _ = guard.context.run(input, ui);
    }

    /// Run the element's context, sending all the queued up events to the context.
    ///
    /// - `ui` is your function used to render the context.
    /// - `time` is the current system monotonic time.
    pub fn render(
        &self,
        renderer: &mut GlowRenderer,
        scale: i32,
        alpha: f32,
        location: Point<i32, Physical>,
        ui: impl FnOnce(&egui::Context),
        time: std::time::Duration,
    ) -> Result<EguiRenderElement, GlesError> {
        let guard = &mut *self.inner.lock().unwrap();

        let size = self.size.to_physical(scale);
        let buffer_size = self.size.to_buffer(scale, Transform::Normal);

        let painter = match guard.painter.as_mut() {
            Some(painter) => painter,
            None => {
                let mut max_texture_size = 0;
                {
                    let gles_renderer: &mut gles::GlesRenderer = renderer.borrow_mut();
                    let _ = gles_renderer.with_context(|gles| unsafe {
                        gles.GetIntegerv(gles::ffi::MAX_TEXTURE_SIZE, &mut max_texture_size);
                    });
                }

                let mut frame = renderer
                    .render(size, Transform::Normal)
                    .map_err(|err| {
                        warn!(?err, "Failed to create egui glow painter for output!");
                        err
                    })
                    .expect("Failed to create frame");

                let painter = frame
                    .with_context(|context| {
                        // SAFETY: In the context of this compositor, the glow renderer/context
                        // lives for 'static, so the pointer to it should always be valid.
                        egui_glow::Painter::new(context.clone(), "", None)
                    })?
                    .map_err(|err| {
                        warn!(?err, "Failed to create egui glow painter for output!");
                        GlesError::ShaderCompileError
                    })?;

                guard.painter.insert(EguiGlowPainter {
                    painter,
                    render_buffer: None,
                    max_texture_size: max_texture_size as usize,
                })
            }
        };

        let _ = painter.render_buffer.take_if(|(s, _)| *s != scale);
        let render_buffer = match painter.render_buffer.as_mut() {
            Some((_, render_buffer)) => render_buffer,
            None => {
                let render_texture: GlesTexture = renderer
                    .create_buffer(Fourcc::Abgr8888, buffer_size)
                    .map_err(|err| {
                        warn!(?err, "Failed to create egui overlay texture buffer!!");
                        err
                    })?;

                let texture_buffer = TextureRenderBuffer::from_texture(
                    renderer,
                    render_texture,
                    scale,
                    Transform::Flipped180, // egui glow painter wants this.
                    None,                  // TODO: Calc opaque regions?
                );

                let render_buffer = painter.render_buffer.insert((scale, texture_buffer));
                &mut render_buffer.1
            }
        };

        let max_texture_size = painter.max_texture_size;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect {
                min: egui::pos2(0.0, 0.0),
                max: egui::pos2(size.w as f32, size.h as f32),
            }),
            time: Some(time.as_secs_f64()),
            predicted_dt: 1.0 / 60.0,
            modifiers: convert_modifiers(guard.last_modifiers),
            events: guard.events.drain(..).collect(),
            hovered_files: Vec::with_capacity(0),
            dropped_files: Vec::with_capacity(0),
            focused: true,
            max_texture_side: Some(max_texture_size),
            pixels_per_point: Some(scale as f32),
            ..Default::default()
        };
        let egui::FullOutput {
            shapes,
            textures_delta,
            ..
        } = guard.context.run(input.clone(), ui);

        render_buffer.render().draw(|texture| {
            renderer.bind(texture.clone())?;
            {
                let mut frame = renderer.render(size, Transform::Normal)?;
                frame.clear([0.; 4], &[Rectangle::from_loc_and_size((0, 0), size)])?;
                painter.painter.paint_and_update_textures(
                    [size.w as u32, size.h as u32],
                    scale as f32,
                    &guard.context.tessellate(shapes),
                    &textures_delta,
                );
            };

            renderer.unbind()?;
            // TODO: Better damage tracking?
            // Without this it leaves weird artifacts from previous frames
            Result::<_, GlesError>::Ok(vec![Rectangle::<i32, Buffer>::from_loc_and_size(
                (0, 0),
                (buffer_size.w, buffer_size.h),
            )])
        })?;

        let texture_element: FhtTextureElement = TextureRenderElement::from_texture_render_buffer(
            location.to_f64(),
            &render_buffer,
            Some(alpha),
            None,
            Some(self.size),
            Kind::Unspecified,
        )
        .into();
        Ok(texture_element.into())
    }
}

pub struct EguiElementInner {
    /// The egui context used to run and draw our UI.
    context: egui::Context,
    /// Glow painter state.
    /// This may not be always Some, as it gets initialized on the first [`Self::render`] call.
    painter: Option<EguiGlowPainter>,

    /// The last pointer position on this element.
    /// If this is `None`, this means there's no pointer on the element.
    last_pointer_position: Option<Point<i32, Logical>>,
    /// Last registered modifiers state for keyboard input events.
    last_modifiers: ModifiersState,
    /// XKB keyboard keymap layout.
    #[allow(unused)] // we have to keep it as long as xkb_state
    xkb_keymap: xkb::Keymap,
    /// XKB keyboard state machine.
    xkb_state: xkb::State,
    /// Queued up events.
    ///
    /// We use egui in "reactive" mode in this integration, and by that we mean that we update our
    /// egui state whenever we redraw/render the overlay UI.
    ///
    /// When we update our state, we drain these events to send them to our
    /// [`Context`](egui::Context)
    events: Vec<egui::Event>,
}

impl std::fmt::Debug for EguiElementInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EguiOutputState")
            .field("context", &self.context)
            .field("painter", &self.painter)
            .field("last_modifiers", &self.last_modifiers)
            .field("xkb_keymap", &"...")
            .field("xkb_state", &"...")
            .field("events", &self.events)
            .finish()
    }
}

impl EguiElementInner {
    /// Send a pointer position event to this context.
    ///
    /// This expects the position to be relative to the egui element.
    pub fn input_event_pointer_position(&mut self, position: Point<i32, Logical>) {
        // NOTE: No need to check for wants_pointer_input since it tries to base this off the
        // pointer position, so it must be updated regardless.
        self.last_pointer_position = Some(position);
        self.events.push(egui::Event::PointerMoved(egui::pos2(
            position.x as f32,
            position.y as f32,
        )));
    }

    /// Send a pointer axis/scroll event to this context.
    ///
    /// If this returns true, don't send the pointer axis event to the wayland client below the
    /// pointer.
    pub fn input_event_pointer_axis(&mut self, x_amount: f64, y_amount: f64) -> bool {
        if !self.context.wants_pointer_input() {
            return false;
        }

        self.events.push(egui::Event::Scroll(egui::vec2(
            x_amount as f32,
            y_amount as f32,
        )));

        true
    }

    /// Send a pointer button event to this context.
    ///
    /// If this returns true, don't send the pointer axis event to the wayland client below the
    /// pointer.
    pub fn input_event_pointer_button(&mut self, button: MouseButton, pressed: bool) -> bool {
        if !self.context.wants_pointer_input() {
            return false;
        }

        let Some(last_pointer_position) = self.last_pointer_position else {
            return false;
        };

        let button = match button {
            MouseButton::Left => egui::PointerButton::Primary,
            MouseButton::Middle => egui::PointerButton::Middle,
            MouseButton::Right => egui::PointerButton::Secondary,
            _ => return false,
        };

        self.events.push(egui::Event::PointerButton {
            pos: egui::pos2(
                last_pointer_position.x as f32,
                last_pointer_position.y as f32,
            ),
            button,
            pressed,
            modifiers: convert_modifiers(self.last_modifiers),
        });

        true
    }

    /// Send a keyboard key event to this context.
    ///
    /// If this returns true, don't send the keyboard event to the wayland client below the
    /// pointer.
    ///
    /// FIXME: This outputs garbage sometimes?
    pub fn input_event_keyboard(
        &mut self,
        key_code: u32,
        pressed: bool,
        modifiers: ModifiersState,
    ) -> bool {
        self.last_modifiers = modifiers;
        if let Some(key) = convert_keysym(key_code) {
            self.events.push(egui::Event::Key {
                key,
                pressed,
                repeat: false,
                modifiers: convert_modifiers(modifiers),
            });
        }

        self.xkb_state.update_key(
            xkb::Keycode::new(key_code),
            match pressed {
                true => xkb::KeyDirection::Down,
                false => xkb::KeyDirection::Up,
            },
        );

        // Pass to egui the text we just inserted.
        if pressed {
            let text = self.xkb_state.key_get_utf8(xkb::Keycode::new(key_code));
            self.events.push(egui::Event::Text(text));
        }

        self.context.wants_keyboard_input()
    }
}

/// An egui glow painter, based on [`egui_glow`], integrated with the smithay rendering pipewire.
pub struct EguiGlowPainter {
    painter: egui_glow::Painter,
    // We need a buffer in which the painter will draw and track damage.
    // This should get invalidated on each scale change
    render_buffer: Option<(i32, TextureRenderBuffer<GlesTexture>)>,
    /// `GL_MAX_TEXTURE_SIZE`
    max_texture_size: usize,
}

impl std::fmt::Debug for EguiGlowPainter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EguiGlowPainter")
            .field("painter", &"...")
            .field("render_buffers", &self.render_buffer)
            .field("max_texture_size", &self.max_texture_size)
            .finish()
    }
}

/// Convert smithay's [`ModifiersState`] to egui's [`Modifiers`](egui::Modifiers)
fn convert_modifiers(modifiers: ModifiersState) -> egui::Modifiers {
    egui::Modifiers {
        alt: modifiers.alt,
        ctrl: modifiers.ctrl,
        shift: modifiers.shift,
        mac_cmd: false,          // we dont support mac here.
        command: modifiers.ctrl, // ^^^
    }
}

/// Convert a raw [`Keysym`] to egui's [`Key`](egui::Key)
fn convert_keysym(raw: u32) -> Option<egui::Key> {
    use egui::Key::*;
    use smithay::input::keyboard::keysyms;

    #[allow(non_upper_case_globals)]
    Some(match raw {
        keysyms::KEY_Down => ArrowDown,
        keysyms::KEY_Left => ArrowLeft,
        keysyms::KEY_Right => ArrowRight,
        keysyms::KEY_Up => ArrowUp,
        keysyms::KEY_Escape => Escape,
        keysyms::KEY_Tab => Tab,
        keysyms::KEY_BackSpace => Backspace,
        keysyms::KEY_Return => Enter,
        keysyms::KEY_space => Space,
        keysyms::KEY_Insert => Insert,
        keysyms::KEY_Delete => Delete,
        keysyms::KEY_Home => Home,
        keysyms::KEY_End => End,
        keysyms::KEY_Page_Up => PageUp,
        keysyms::KEY_Page_Down => PageDown,
        keysyms::KEY_0 => Num0,
        keysyms::KEY_1 => Num1,
        keysyms::KEY_2 => Num2,
        keysyms::KEY_3 => Num3,
        keysyms::KEY_4 => Num4,
        keysyms::KEY_5 => Num5,
        keysyms::KEY_6 => Num6,
        keysyms::KEY_7 => Num7,
        keysyms::KEY_8 => Num8,
        keysyms::KEY_9 => Num9,
        keysyms::KEY_A => A,
        keysyms::KEY_B => B,
        keysyms::KEY_C => C,
        keysyms::KEY_D => D,
        keysyms::KEY_E => E,
        keysyms::KEY_F => F,
        keysyms::KEY_G => G,
        keysyms::KEY_H => H,
        keysyms::KEY_I => I,
        keysyms::KEY_J => J,
        keysyms::KEY_K => K,
        keysyms::KEY_L => L,
        keysyms::KEY_M => M,
        keysyms::KEY_N => N,
        keysyms::KEY_O => O,
        keysyms::KEY_P => P,
        keysyms::KEY_Q => Q,
        keysyms::KEY_R => R,
        keysyms::KEY_S => S,
        keysyms::KEY_T => T,
        keysyms::KEY_U => U,
        keysyms::KEY_V => V,
        keysyms::KEY_W => W,
        keysyms::KEY_X => X,
        keysyms::KEY_Y => Y,
        keysyms::KEY_Z => Z,
        _ => return None,
    })
}

crate::fht_render_elements! {
    EguiRenderElement => {
        Texture = FhtTextureElement,
    }
}
