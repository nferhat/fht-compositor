use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use egui::{Context, Event, FullOutput, PlatformOutput, Pos2, RawInput, Rect, Vec2};
use egui_glow::Painter;
use smithay::backend::allocator::Fourcc;
use smithay::backend::input::{ButtonState, Device, DeviceCapability, KeyState, MouseButton};
use smithay::backend::renderer::element::texture::{TextureRenderBuffer, TextureRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::{GlesError, GlesTexture};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::{Bind, Frame, Offscreen, Renderer, Unbind};
use smithay::desktop::space::RenderZindex;
#[cfg(feature = "desktop_integration")]
use smithay::desktop::space::SpaceElement;
use smithay::input::keyboard::{KeyboardTarget, KeysymHandle, ModifiersState};
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
    GestureSwipeUpdateEvent, MotionEvent, PointerTarget, RelativeMotionEvent,
};
use smithay::input::{Seat, SeatHandler};
use smithay::utils::{IsAlive, Logical, Physical, Point, Rectangle, Serial, Size, Transform};
use xkbcommon::xkb::Keycode;

mod input;
pub use self::input::{convert_button, convert_key, convert_modifiers};

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
    last_pointer_position: Point<i32, Logical>,
    area: Rectangle<i32, Logical>,
    last_modifiers: ModifiersState,
    last_output: Option<PlatformOutput>,
    pressed: Vec<(Option<egui::Key>, Keycode)>,
    focused: bool,
    events: Vec<Event>,
    kbd: Option<input::KbdInternal>,
    #[cfg(feature = "desktop_integration")]
    z_index: u8,
}

impl fmt::Debug for EguiInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("EguiInner");
        d.field("pointers", &self.pointers)
            .field("last_pointer_position", &self.last_pointer_position)
            .field("area", &self.area)
            .field("last_modifiers", &self.last_modifiers)
            .field("last_output", &self.last_output.as_ref().map(|_| "..."))
            .field("pressed", &self.pressed)
            .field("focused", &self.focused)
            .field("events", &self.events)
            .field("kbd", &self.kbd);

        #[cfg(feature = "desktop_integration")]
        {
            d.field("z_index", &self.z_index);
        }

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
    pub fn new(area: Rectangle<i32, Logical>) -> EguiState {
        EguiState {
            ctx: Context::default(),
            start_time: Instant::now(),
            inner: Arc::new(Mutex::new(EguiInner {
                pointers: 0,
                last_pointer_position: (0, 0).into(),
                area,
                last_modifiers: ModifiersState::default(),
                last_output: None,
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
                #[cfg(feature = "desktop_integration")]
                z_index: RenderZindex::Overlay as u8,
            })),
        }
    }

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
        inner.last_pointer_position = position;
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
            let last_pos = inner.last_pointer_position;
            let modifiers = convert_modifiers(inner.last_modifiers);
            inner.events.push(Event::PointerButton {
                pos: Pos2::new(last_pos.x as f32, last_pos.y as f32),
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

    // TODO: touch inputs

    /// Produce a new frame of egui. Returns a [`RenderElement`]
    ///
    /// - `ui` is your drawing function
    /// - `renderer` is a [`GlowRenderer`]
    /// - `area` limits the space egui will be using and offsets the result
    /// - `scale` is the scale egui should render in
    /// - `alpha` applies (additional) transparency to the whole ui
    /// - `start_time` need to be a fixed point in time before the first `run` call to measure
    ///   animation-times and the like.
    /// - `modifiers` should be the current state of modifiers pressed on the keyboards.
    pub fn render(
        &self,
        ui: impl FnOnce(&Context),
        renderer: &mut GlowRenderer,
        area: Rectangle<i32, Logical>,
        scale: f64,
        alpha: f32,
    ) -> Result<TextureRenderElement<GlesTexture>, GlesError> {
        let int_scale = scale.ceil() as i32;
        let user_data = renderer.egl_context().user_data();
        if user_data.get::<UserDataType>().is_none() {
            let painter = {
                let mut frame = renderer.render(
                    area.size.to_physical(int_scale),
                    smithay::utils::Transform::Normal,
                )?;
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

        let mut inner = self.inner.lock().unwrap();
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
                .create_buffer(
                    Fourcc::Abgr8888,
                    area.size
                        .to_buffer(int_scale, smithay::utils::Transform::Normal),
                )
                .expect("Failed to create buffer");
            TextureRenderBuffer::from_texture(
                renderer,
                render_texture,
                int_scale,
                Transform::Flipped180,
                None,
            )
        });

        let screen_size: Size<i32, Physical> = area.size.to_physical(int_scale);
        let input = RawInput {
            screen_rect: Some(Rect {
                min: Pos2 { x: 0.0, y: 0.0 },
                max: Pos2 {
                    x: screen_size.w as f32,
                    y: screen_size.h as f32,
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
            platform_output,
            shapes,
            textures_delta,
            ..
        } = self.ctx.run(input.clone(), ui);
        inner.last_output = Some(platform_output);

        let needs_recreate = inner.area != area;
        inner.area = area;

        if needs_recreate {
            *render_buffer = {
                let render_texture = renderer.create_buffer(
                    Fourcc::Abgr8888,
                    area.size
                        .to_buffer(int_scale, smithay::utils::Transform::Normal),
                )?;
                TextureRenderBuffer::from_texture(
                    renderer,
                    render_texture,
                    int_scale,
                    Transform::Flipped180,
                    None,
                )
            };
        }

        render_buffer.render().draw(|tex| {
            renderer.bind(tex.clone())?;
            let physical_area = area.to_physical(int_scale);
            {
                let mut frame = renderer.render(physical_area.size, Transform::Normal)?;
                frame.clear([0.0, 0.0, 0.0, 0.0], &[physical_area])?;
                painter.paint_and_update_textures(
                    [physical_area.size.w as u32, physical_area.size.h as u32],
                    int_scale as f32,
                    &self.ctx.tessellate(shapes),
                    &textures_delta,
                );
            }
            renderer.unbind()?;

            let used = self.ctx.used_rect();
            let margin = self.ctx.style().visuals.clip_rect_margin.ceil() as i32;
            let window_shadow = self.ctx.style().visuals.window_shadow.extrusion.ceil() as i32;
            let popup_shadow = self.ctx.style().visuals.popup_shadow.extrusion.ceil() as i32;
            let offset = margin + Ord::max(window_shadow, popup_shadow);
            Result::<_, GlesError>::Ok(vec![Rectangle::<i32, Logical>::from_extemities(
                (
                    (used.min.x.floor() as i32).saturating_sub(offset),
                    (used.min.y.floor() as i32).saturating_sub(offset),
                ),
                (
                    (used.max.x.ceil() as i32) + (offset * 2),
                    (used.max.y.ceil() as i32) + (offset * 2),
                ),
            )
            .to_buffer(int_scale, Transform::Flipped180, &area.size)])
        })?;

        Ok(TextureRenderElement::from_texture_render_buffer(
            area.loc.to_f64().to_physical(scale),
            &render_buffer,
            Some(alpha),
            None,
            None,
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
                    .render((1, 1).into(), smithay::utils::Transform::Normal)
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
                    .render((1, 1).into(), smithay::utils::Transform::Normal)
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

    /// Sets the z_index as reported by [`SpaceElement::z_index`].
    ///
    /// The default is [`RenderZindex::Overlay`].
    #[cfg(feature = "desktop_integration")]
    pub fn set_zindex(&self, idx: u8) {
        self.inner.lock().unwrap().z_index = idx;
    }

    /// Returns the egui [`PlatformOutput`] generated by the last [`Self::render`] call
    pub fn last_output(&self) -> Option<PlatformOutput> {
        self.inner.lock().unwrap().last_output.take()
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

#[cfg(feature = "desktop_integration")]
impl SpaceElement for EguiState {
    fn bbox(&self) -> Rectangle<i32, Logical> {
        self.inner.lock().unwrap().area
    }

    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        let pos: Point<i32, _> = point.to_i32_round();
        let last_pos = self.inner.lock().unwrap().last_pointer_position;
        if (pos.x - last_pos.x) + (pos.y - last_pos.y) < 10 {
            self.wants_pointer()
        } else {
            false
        }
    }

    fn set_activate(&self, _activated: bool) {}
    fn output_enter(&self, _output: &smithay::output::Output, _overlap: Rectangle<i32, Logical>) {}
    fn output_leave(&self, _output: &smithay::output::Output) {}

    fn z_index(&self) -> u8 {
        self.inner.lock().unwrap().z_index as u8
    }
}
