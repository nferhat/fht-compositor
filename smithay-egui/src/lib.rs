use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use egui::{Context, Event, FullOutput, Pos2, RawInput, Rect, Vec2};
use egui_glow::Painter;
use smithay::backend::allocator::Fourcc;
use smithay::backend::input::{ButtonState, Device, DeviceCapability, KeyState, MouseButton};
use smithay::backend::renderer::element::texture::{TextureRenderBuffer, TextureRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::{GlesError, GlesTexture};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::{Bind, Frame, Offscreen, Renderer, Unbind};
use smithay::input::keyboard::{KeyboardTarget, KeysymHandle, ModifiersState};
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
    GestureSwipeUpdateEvent, MotionEvent, PointerTarget, RelativeMotionEvent,
};
use smithay::input::{Seat, SeatHandler};
use smithay::utils::{Buffer, IsAlive, Logical, Point, Rectangle, Serial, Size, Transform};
use xkbcommon::xkb::Keycode;

mod input;
pub use self::input::{convert_button, convert_key, convert_modifiers};

// Reexport egui for dependencies to use it
pub use egui;
pub use egui_glow;
pub use egui_extras;

/// smithay-egui state object
#[derive(Debug, Clone)]
pub struct EguiState {
    inner: Arc<Mutex<EguiInner>>,
    ctx: Context,
    start_time: Instant,
}

impl PartialEq for EguiState {
    fn eq(&self, other: &Self) -> bool {
        self.ctx == other.ctx
    }
}

struct EguiInner {
    pointers: usize,
    size: Size<i32, Logical>,
    needs_new_buffer: bool,
    last_modifiers: ModifiersState,
    pressed: Vec<(Option<egui::Key>, Keycode)>,
    focused: bool,
    events: Vec<Event>,
    kbd: Option<input::KbdInternal>,
}

impl fmt::Debug for EguiInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("EguiInner");
        d.field("pointers", &self.pointers)
            .field("area", &self.size)
            .field("needs_new_buffer", &self.needs_new_buffer)
            .field("last_modifiers", &self.last_modifiers)
            .field("pressed", &self.pressed)
            .field("focused", &self.focused)
            .field("events", &self.events)
            .field("kbd", &self.kbd);

        d.finish()
    }
}

struct GlState {
    painter: Painter,
    render_buffers: HashMap<usize, TextureRenderBuffer<GlesTexture>>,
    #[cfg(feature = "image")]
    images: HashMap<String, egui_extras::image::RetainedImage>,
}
type UserDataType = Rc<RefCell<GlState>>;

impl EguiState {
    /// Creates a new `EguiState`
    pub fn new(size: Size<i32, Logical>) -> EguiState {
        EguiState {
            ctx: Context::default(),
            start_time: Instant::now(),
            inner: Arc::new(Mutex::new(EguiInner {
                pointers: 0,
                size,
                needs_new_buffer: false,
                last_modifiers: ModifiersState::default(),
                events: Vec::new(),
                focused: false,
                pressed: Vec::new(),
                kbd: match input::KbdInternal::new() {
                    Some(kbd) => Some(kbd),
                    None => {
                        log::error!("Failed to initialize keymap for text input in egui.");
                        None
                    }
                },
            })),
        }
    }

    /// Get the unique identifier of this [`EguiState`]
    fn id(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }

    /// Retrieve the underlying [`egui::Context`]
    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// If true, egui is currently listening on text input (e.g. typing text in a TextEdit).
    pub fn wants_keyboard(&self) -> bool {
        self.ctx.wants_keyboard_input()
    }

    /// True if egui is currently interested in the pointer (mouse or touch).
    /// Could be the pointer is hovering over a Window or the user is dragging a widget.
    /// If false, the pointer is outside of any egui area and so you may want to forward it to other
    /// clients as usual. Returns false if a drag started outside of egui and then moved over an
    /// egui area.
    pub fn wants_pointer(&self) -> bool {
        self.ctx.wants_pointer_input()
    }

    /// Pass new input devices to `EguiState` for internal tracking
    pub fn handle_device_added(&self, device: &impl Device) {
        if device.has_capability(DeviceCapability::Pointer) {
            self.inner.lock().unwrap().pointers += 1;
        }
    }

    /// Remove input devices to `EguiState` for internal tracking
    pub fn handle_device_removed(&self, device: &impl Device) {
        let mut inner = self.inner.lock().unwrap();
        if device.has_capability(DeviceCapability::Pointer) {
            inner.pointers -= 1;
        }
        if inner.pointers == 0 {
            inner.events.push(Event::PointerGone);
        }
    }

    /// Pass keyboard events into `EguiState`.
    ///
    /// You do not want to pass in events, egui should not react to, but you need to make sure they
    /// add up. So for every pressed event, you want to send a released one.
    ///
    /// You likely want to use the filter-closure of
    /// [`smithay::wayland::seat::KeyboardHandle::input`] to optain these values.
    /// Use [`smithay::wayland::seat::KeysymHandle`] and the provided
    /// [`smithay::wayland::seat::ModifiersState`].
    pub fn handle_keyboard(&self, handle: &KeysymHandle, pressed: bool, modifiers: ModifiersState) {
        let mut inner = self.inner.lock().unwrap();
        inner.last_modifiers = modifiers;
        let key = if let Some(key) = convert_key(handle.raw_syms().iter().copied()) {
            inner.events.push(Event::Key {
                key,
                pressed,
                repeat: false,
                modifiers: convert_modifiers(modifiers),
            });
            Some(key)
        } else {
            None
        };

        if pressed {
            inner.pressed.push((key, handle.raw_code()));
        } else {
            inner.pressed.retain(|(_, code)| code != &handle.raw_code());
        }

        if let Some(kbd) = inner.kbd.as_mut() {
            kbd.key_input(handle.raw_code().raw(), pressed);

            if pressed {
                let utf8 = kbd.get_utf8(handle.raw_code().raw());
                /* utf8 contains the utf8 string generated by that keystroke
                 * it can contain 1, multiple characters, or even be empty
                 */
                inner.events.push(Event::Text(utf8));
            }
        }
    }

    /// Pass new pointer coordinates to `EguiState`
    pub fn handle_pointer_motion(&self, position: Point<i32, Logical>) {
        let mut inner = self.inner.lock().unwrap();
        inner.events.push(Event::PointerMoved(Pos2::new(
            position.x as f32,
            position.y as f32,
        )))
    }

    /// Pass pointer button presses to `EguiState`
    ///
    /// Note: If you are unsure about *which* PointerButtonEvents to send to smithay-egui
    ///       instead of normal clients, check [`EguiState::wants_pointer`] to figure out,
    ///       if there is an egui-element below your pointer.
    pub fn handle_pointer_button(&self, button: MouseButton, pressed: bool) {
        if let Some(button) = convert_button(button) {
            let mut inner = self.inner.lock().unwrap();
            let modifiers = convert_modifiers(inner.last_modifiers);
            inner.events.push(Event::PointerButton {
                // SAFETY: We don't support touch here, so this should always be Some
                pos: self.ctx.pointer_latest_pos().unwrap(),
                button,
                pressed,
                modifiers,
            })
        }
    }

    /// Pass a pointer axis scrolling to `EguiState`
    ///
    /// Note: If you are unsure about *which* PointerAxisEvents to send to smithay-egui
    ///       instead of normal clients, check [`EguiState::wants_pointer`] to figure out,
    ///       if there is an egui-element below your pointer.
    pub fn handle_pointer_axis(&self, x_amount: f64, y_amount: f64) {
        self.inner.lock().unwrap().events.push(Event::Scroll(Vec2 {
            x: x_amount as f32,
            y: y_amount as f32,
        }))
    }

    /// Set if this [`EguiState`] should consider itself focused
    pub fn set_focused(&self, focused: bool) {
        self.inner.lock().unwrap().focused = focused;
    }

    /// Resize the area of the egui context.
    pub fn resize(&self, area: Size<i32, Logical>) {
        let mut inner = self.inner.lock().unwrap();
        let old_size = inner.size;
        inner.size = area;
        inner.needs_new_buffer = old_size != area;
    }

    // TODO: touch inputs

    /// Run the inner context for one frame.
    pub fn run(&self, ui: impl FnOnce(&Context), scale: f64) {
        let int_scale = scale.ceil() as i32;
        let mut inner = self.inner.lock().unwrap();
        let output_size = inner.size.to_physical(int_scale);

        let input = RawInput {
            screen_rect: Some(Rect {
                min: Pos2 { x: 0.0, y: 0.0 },
                max: Pos2 {
                    x: output_size.w as f32,
                    y: output_size.h as f32,
                },
            }),
            pixels_per_point: Some(int_scale as f32),
            time: Some(self.start_time.elapsed().as_secs_f64()),
            predicted_dt: 1.0 / 60.0,
            modifiers: convert_modifiers(inner.last_modifiers),
            events: inner.events.drain(..).collect(),
            hovered_files: Vec::with_capacity(0),
            dropped_files: Vec::with_capacity(0),
            focused: inner.focused,
            max_texture_side: None, // TODO query from GlState somehow
        };

        let _ = self.context().run(input, ui);
    }

    /// Produce a new frame of egui. Returns a [`RenderElement`]
    ///
    /// - `ui` is your drawing function
    /// - `renderer` is a [`GlowRenderer`]
    /// - `location` limits the location of the final Egui output
    /// - `scale` is the scale egui should render in
    /// - `alpha` applies (additional) transparency to the whole ui
    /// - `start_time` need to be a fixed point in time before the first `run` call to measure
    ///   animation-times and the like.
    /// - `modifiers` should be the current state of modifiers pressed on the keyboards.
    pub fn render(
        &self,
        ui: impl FnOnce(&Context),
        renderer: &mut GlowRenderer,
        location: Point<i32, Logical>,
        scale: f64,
        alpha: f32,
    ) -> Result<TextureRenderElement<GlesTexture>, GlesError> {
        let mut inner = self.inner.lock().unwrap();
        let int_scale = scale.ceil() as i32;
        let output_size = inner.size.to_physical(int_scale);
        let buffer_size = inner.size.to_buffer(int_scale, Transform::Normal);

        let user_data = renderer.egl_context().user_data();
        if user_data.get::<UserDataType>().is_none() {
            let painter = {
                let mut frame = renderer.render(output_size, Transform::Normal)?;
                frame
                    .with_context(|context| Painter::new(context.clone(), "", None))?
                    .map_err(|_| GlesError::ShaderCompileError)?
            };
            renderer.egl_context().user_data().insert_if_missing(|| {
                UserDataType::new(RefCell::new(GlState {
                    painter,
                    render_buffers: HashMap::new(),
                    #[cfg(feature = "image")]
                    images: HashMap::new(),
                }))
            });
        }

        let gl_state = renderer
            .egl_context()
            .user_data()
            .get::<UserDataType>()
            .unwrap()
            .clone();
        let mut borrow = gl_state.borrow_mut();
        let &mut GlState {
            ref mut painter,
            ref mut render_buffers,
            ..
        } = &mut *borrow;

        let render_buffer = render_buffers.entry(self.id()).or_insert_with(|| {
            let render_texture = renderer
                .create_buffer(Fourcc::Abgr8888, buffer_size)
                .expect("Failed to create buffer");
            TextureRenderBuffer::from_texture(
                renderer,
                render_texture,
                int_scale,
                Transform::Flipped180,
                None,
            )
        });

        if inner.needs_new_buffer {
            inner.needs_new_buffer = false;
            *render_buffer = {
                let render_texture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
                TextureRenderBuffer::from_texture(
                    renderer,
                    render_texture,
                    int_scale,
                    Transform::Flipped180,
                    None,
                )
            };
        }

        let input = RawInput {
            screen_rect: Some(Rect {
                min: Pos2 { x: 0.0, y: 0.0 },
                max: Pos2 {
                    x: output_size.w as f32,
                    y: output_size.h as f32,
                },
            }),
            pixels_per_point: Some(int_scale as f32),
            time: Some(self.start_time.elapsed().as_secs_f64()),
            predicted_dt: 1.0 / 60.0,
            modifiers: convert_modifiers(inner.last_modifiers),
            events: inner.events.drain(..).collect(),
            hovered_files: Vec::with_capacity(0),
            dropped_files: Vec::with_capacity(0),
            focused: inner.focused,
            max_texture_side: Some(painter.max_texture_side()), // TODO query from GlState somehow
        };

        let FullOutput {
            shapes,
            textures_delta,
            ..
        } = self.ctx.run(input.clone(), ui);

        render_buffer.render().draw(|tex| {
            renderer.bind(tex.clone())?;
            {
                let mut frame = renderer.render(output_size, Transform::Normal)?;
                frame.clear(
                    [0.0, 0.0, 0.0, 0.0],
                    &[Rectangle::from_loc_and_size(
                        location.to_physical(int_scale),
                        output_size,
                    )],
                )?;
                painter.paint_and_update_textures(
                    [output_size.w as u32, output_size.h as u32],
                    int_scale as f32,
                    &self.ctx.tessellate(shapes),
                    &textures_delta,
                );
            }
            renderer.unbind()?;

            // TODO: Better damage tracking?
            // Without this it leaves weird artifacts from previous frames
            Result::<_, GlesError>::Ok(vec![Rectangle::<i32, Buffer>::from_loc_and_size(
                (0, 0),
                (buffer_size.w, buffer_size.h),
            )])
        })?;

        Ok(TextureRenderElement::from_texture_render_buffer(
            location.to_f64().to_physical(scale),
            &render_buffer,
            Some(alpha),
            None,
            Some(inner.size),
            Kind::Unspecified,
        ))
    }

    #[cfg(all(feature = "image", any(feature = "png", feature = "jpg")))]
    pub fn load_image(
        &self,
        renderer: &mut GlowRenderer,
        name: String,
        bytes: &[u8],
    ) -> Result<(), String> {
        let user_data = renderer.egl_context().user_data();
        if user_data.get::<UserDataType>().is_none() {
            let painter = {
                let mut frame = renderer
                    .render((1, 1).into(), Transform::Normal)
                    .map_err(|err| format!("{}", err))?;
                frame
                    .with_context(|context| Painter::new(context.clone(), "", None))
                    .map_err(|err| format!("{}", err))??
            };
            renderer.egl_context().user_data().insert_if_missing(|| {
                UserDataType::new(RefCell::new(GlState {
                    painter,
                    render_buffers: HashMap::new(),
                    #[cfg(feature = "image")]
                    images: HashMap::new(),
                }))
            });
        }

        let gl_state = renderer
            .egl_context()
            .user_data()
            .get::<UserDataType>()
            .unwrap()
            .clone();
        let mut borrow = gl_state.borrow_mut();

        let image = egui_extras::RetainedImage::from_image_bytes(name.clone(), bytes)?;
        borrow.images.insert(name, image);

        Ok(())
    }

    #[cfg(all(feature = "image", feature = "svg"))]
    pub fn load_svg(
        &self,
        renderer: &mut GlowRenderer,
        name: String,
        bytes: &[u8],
    ) -> Result<(), String> {
        let user_data = renderer.egl_context().user_data();
        if user_data.get::<UserDataType>().is_none() {
            let painter = {
                let mut frame = renderer
                    .render((1, 1).into(), Transform::Normal)
                    .map_err(|err| format!("{}", err))?;
                frame
                    .with_context(|context| Painter::new(context.clone(), "", None))
                    .map_err(|err| format!("{}", err))??
            };
            renderer.egl_context().user_data().insert_if_missing(|| {
                UserDataType::new(RefCell::new(GlState {
                    painter,
                    render_buffers: HashMap::new(),
                    #[cfg(feature = "image")]
                    images: HashMap::new(),
                }))
            });
        }

        let gl_state = renderer
            .egl_context()
            .user_data()
            .get::<UserDataType>()
            .unwrap()
            .clone();
        let mut borrow = gl_state.borrow_mut();

        let image = egui_extras::RetainedImage::from_svg_bytes(name.clone(), bytes)?;
        borrow.images.insert(name, image);

        Ok(())
    }

    #[cfg(feature = "image")]
    pub fn with_image<F, R>(&self, renderer: &mut GlowRenderer, name: &str, closure: F) -> Option<R>
    where
        F: FnOnce(&egui_extras::RetainedImage, &Context) -> R,
    {
        let user_data = renderer.egl_context().user_data();
        let state = user_data.get::<UserDataType>()?;
        let state_ref = state.borrow();
        let img = state_ref.images.get(name)?;
        Some(closure(img, &self.ctx))
    }
}

impl IsAlive for EguiState {
    fn alive(&self) -> bool {
        true
    }
}

impl<D: SeatHandler> PointerTarget<D> for EguiState {
    fn enter(&self, _seat: &Seat<D>, _data: &mut D, event: &MotionEvent) {
        self.handle_pointer_motion(event.location.to_i32_floor())
    }

    fn motion(&self, _seat: &Seat<D>, _data: &mut D, event: &MotionEvent) {
        self.handle_pointer_motion(event.location.to_i32_round())
    }

    fn relative_motion(&self, _seat: &Seat<D>, _data: &mut D, _event: &RelativeMotionEvent) {}

    fn button(&self, _seat: &Seat<D>, _data: &mut D, event: &ButtonEvent) {
        if let Some(button) = match event.button {
            0x110 => Some(MouseButton::Left),
            0x111 => Some(MouseButton::Right),
            0x112 => Some(MouseButton::Middle),
            0x115 => Some(MouseButton::Forward),
            0x116 => Some(MouseButton::Back),
            _ => None,
        } {
            self.handle_pointer_button(button, event.state == ButtonState::Pressed)
        }
    }

    fn axis(&self, _seat: &Seat<D>, _data: &mut D, _frame: AxisFrame) {
        // TODO
        //self.handle_pointer_axis(frame., y_amount)
    }

    fn leave(&self, _seat: &Seat<D>, _data: &mut D, _serial: Serial, _time: u32) {}

    fn frame(&self, _seat: &Seat<D>, _data: &mut D) {}

    fn gesture_swipe_begin(&self, _seat: &Seat<D>, _data: &mut D, _event: &GestureSwipeBeginEvent) {
    }

    fn gesture_swipe_update(
        &self,
        _seat: &Seat<D>,
        _data: &mut D,
        _event: &GestureSwipeUpdateEvent,
    ) {
    }

    fn gesture_swipe_end(&self, _seat: &Seat<D>, _data: &mut D, _event: &GestureSwipeEndEvent) {}

    fn gesture_pinch_begin(&self, _seat: &Seat<D>, _data: &mut D, _event: &GesturePinchBeginEvent) {
    }

    fn gesture_pinch_update(
        &self,
        _seat: &Seat<D>,
        _data: &mut D,
        _event: &GesturePinchUpdateEvent,
    ) {
    }

    fn gesture_pinch_end(&self, _seat: &Seat<D>, _data: &mut D, _event: &GesturePinchEndEvent) {}

    fn gesture_hold_begin(&self, _seat: &Seat<D>, _data: &mut D, _event: &GestureHoldBeginEvent) {}

    fn gesture_hold_end(&self, _seat: &Seat<D>, _data: &mut D, _event: &GestureHoldEndEvent) {}
}

impl<D: SeatHandler> KeyboardTarget<D> for EguiState {
    fn enter(&self, _seat: &Seat<D>, _data: &mut D, keys: Vec<KeysymHandle<'_>>, _serial: Serial) {
        self.set_focused(true);

        let mut inner = self.inner.lock().unwrap();
        for handle in &keys {
            let key = if let Some(key) = convert_key(handle.raw_syms().iter().copied()) {
                let modifiers = convert_modifiers(inner.last_modifiers);
                inner.events.push(Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                });
                Some(key)
            } else {
                None
            };
            inner.pressed.push((key, handle.raw_code()));
            if let Some(kbd) = inner.kbd.as_mut() {
                kbd.key_input(handle.raw_code().raw(), true);
            }
        }
    }

    fn leave(&self, _seat: &Seat<D>, _data: &mut D, _serial: Serial) {
        self.set_focused(false);

        let keys = std::mem::take(&mut self.inner.lock().unwrap().pressed);
        let mut inner = self.inner.lock().unwrap();
        for (key, code) in keys {
            if let Some(key) = key {
                let modifiers = convert_modifiers(inner.last_modifiers);
                inner.events.push(Event::Key {
                    key,
                    pressed: false,
                    repeat: false,
                    modifiers,
                });
            }
            if let Some(kbd) = inner.kbd.as_mut() {
                kbd.key_input(code.raw(), false);
            }
        }
    }

    fn key(
        &self,
        _seat: &Seat<D>,
        _data: &mut D,
        key: KeysymHandle<'_>,
        state: KeyState,
        _serial: Serial,
        _time: u32,
    ) {
        let modifiers = self.inner.lock().unwrap().last_modifiers;
        self.handle_keyboard(&key, state == KeyState::Pressed, modifiers)
    }

    fn modifiers(
        &self,
        _seat: &Seat<D>,
        _data: &mut D,
        modifiers: ModifiersState,
        _serial: Serial,
    ) {
        self.inner.lock().unwrap().last_modifiers = modifiers;
    }
}
