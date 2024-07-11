//! Egui implementation for the compositor.
//!
//! A lot of thd code ideas and impls are from
//! [smithay-egui](https://github.com/smithay/smithay-egui), with additional tailoring to fit the
//! compositor's needs.

use std::borrow::BorrowMut;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use smithay::backend::allocator::Fourcc;
use smithay::backend::input::{Device, DeviceCapability, MouseButton};
use smithay::backend::renderer::element::texture::{TextureRenderBuffer, TextureRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::{self, GlesError, GlesTexture};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::{Bind, Frame, Offscreen, Renderer, Unbind};
use smithay::input::keyboard::{xkb, ModifiersState};
use smithay::output::Output;
use smithay::utils::{Buffer, Point, Rectangle, Transform};

use crate::renderer::texture_element::FhtTextureElement;
use crate::utils::geometry::{Local, RectGlobalExt, SizeExt};
use crate::utils::output::OutputExt;

/// Egui debug overlays state.
///
/// This will manage the egui state of each output this gets registered on.
///
/// For input to work, you hagve to hook your compositor input management and use the associated
/// [`Self::handle_input_event`] function to let egui know of your input.
#[derive(Debug, Default)]
pub struct Egui {
    /// The registered outputs.
    pub outputs: IndexMap<Output, Arc<Mutex<EguiOverlay>>>,
    /// Whether the overlays are active or not.
    ///
    /// This is false, handling of inputs will be ignored, and rendering will not be effective.
    pub active: bool,
}

impl Egui {
    /// Create an overlay for this output.
    pub fn add_output(
        &mut self,
        output: Output,
        pointer_devices: usize,
        modifiers: ModifiersState,
    ) {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let xkb_keymap = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            "",
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("Failed to create XKB keymap from constants?");
        let xkb_state = xkb::State::new(&xkb_keymap);

        let state = EguiOverlay {
            output: output.clone(),
            context: egui::Context::default(),
            painter: None, // initialized on first draw call
            pointer_devices,
            last_pointer_position: Point::default(),
            focused: false,
            xkb_keymap,
            xkb_state,
            last_modifiers: modifiers,
            events: vec![],
        };

        assert!(
            self.outputs
                .insert(output, Arc::new(Mutex::new(state)))
                .is_none(),
            "Can't register egui overlay for an output twice!"
        );
    }
}

pub struct EguiOverlay {
    /// The associated output.
    output: Output,
    /// The egui context used to run and draw our UI.
    context: egui::Context,
    /// Glow painter state.
    ///
    /// This may not be always Some, as it gets initialized on the first [`Self::render`] call.
    painter: Option<EguiGlowPainter>,

    /// How many pointer devices do we have.
    pointer_devices: usize,
    /// The last registered pointer position of this overlay, local to the output its being drawn
    /// on.
    last_pointer_position: Point<i32, Local>,
    /// Whether we are focused.
    focused: bool,

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

impl std::fmt::Debug for EguiOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EguiOutputState")
            .field("output", &self.output.name())
            .field("context", &self.context)
            .field("painter", &self.painter)
            .field("pointer_devices", &self.pointer_devices)
            .field("focused", &self.focused)
            .field("last_modifiers", &self.last_modifiers)
            .field("xkb_keymap", &"...")
            .field("xkb_state", &"...")
            .field("events", &self.events)
            .finish()
    }
}

impl EguiOverlay {
    /// Send a device added event to this context.
    pub fn input_event_device_added(&mut self, device: &impl Device) {
        self.pointer_devices += device.has_capability(DeviceCapability::Pointer) as usize;
    }

    /// Send a device removed event to this context.
    pub fn input_event_device_removed(&mut self, device: &impl Device) {
        self.pointer_devices -= device.has_capability(DeviceCapability::Pointer) as usize;
        if self.pointer_devices > 0 {
            self.events.push(egui::Event::PointerGone)
        }
    }

    /// Send a pointer position event to this context.
    ///
    /// This expects the position to be relative to this output.
    pub fn input_event_pointer_position(&mut self, position: Point<i32, Local>) {
        // NOTE: No need to check for wants_pointer_input since it tries to base this off the
        // pointer position, so it must be updated regardless.
        self.last_pointer_position = position;
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

        let button = match button {
            MouseButton::Left => egui::PointerButton::Primary,
            MouseButton::Middle => egui::PointerButton::Primary,
            MouseButton::Right => egui::PointerButton::Primary,
            _ => return false,
        };

        self.events.push(egui::Event::PointerButton {
            pos: egui::pos2(
                self.last_pointer_position.x as f32,
                self.last_pointer_position.y as f32,
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

    /// Run this context for a single frame.
    ///
    /// This will dispatch all the queued events to the context.
    pub fn run(&mut self, ui: impl FnOnce(&egui::Context), time: std::time::Duration, scale: i32) {
        let output_size = self.output.geometry().size.as_logical().to_physical(scale);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect {
                min: egui::pos2(0.0, 0.0),
                max: egui::pos2(output_size.w as f32, output_size.h as f32),
            }),
            time: Some(time.as_secs_f64()),
            predicted_dt: 1.0 / 60.0,
            modifiers: convert_modifiers(self.last_modifiers),
            events: self.events.drain(..).collect(),
            hovered_files: Vec::with_capacity(0),
            dropped_files: Vec::with_capacity(0),
            focused: true,
            max_texture_side: self
                .painter
                .as_ref()
                .map(|painter| painter.max_texture_size),
            ..Default::default()
        };

        let _ = self.context.run(input, ui);
    }

    /// Produce a new frame of the overlay and render it inside a render element.
    ///
    /// If you need to only run the overlay without rendering it, see [`Self::run`]
    pub fn render(
        &mut self,
        ui: impl FnOnce(&egui::Context),
        renderer: &mut GlowRenderer,
        scale: f64,
        alpha: f32,
        time: std::time::Duration,
    ) -> Result<FhtTextureElement, GlesError> {
        let id = std::ptr::addr_of!(*self) as usize;
        let int_scale = scale.ceil() as i32;
        let output_geo = self.output.geometry().as_logical();
        let output_size = output_geo.size.to_physical(int_scale);
        let buffer_size = output_geo.size.to_buffer(int_scale, Transform::Normal);

        // Init painter.
        let painter = match self.painter.as_mut() {
            Some(painter) => painter,
            None => {
                let mut max_texture_size = 0;
                {
                    let gles_renderer: &mut gles::GlesRenderer = renderer.borrow_mut();
                    let _ = gles_renderer.with_context(|gles| unsafe {
                        gles.GetIntegerv(gles::ffi::MAX_TEXTURE_SIZE, &mut max_texture_size);
                    });
                }
                dbg!(max_texture_size);

                let mut frame = renderer
                    .render(output_size, Transform::Normal)
                    .map_err(|err| {
                        warn!(?err, "Failed to create egui glow painter for output!");
                        err
                    })?;

                let painter = frame
                    .with_context(|context| {
                        // SAFETY: In the context of this compositor, the glow renderer/context
                        // lives for 'static, so the pointer to it should
                        // always be valid.
                        egui_glow::Painter::new(context.clone(), "", None)
                    })?
                    .map_err(|err| {
                        warn!(?err, "Failed to create egui glow painter for output!");
                        GlesError::ShaderCompileError
                    })?;

                self.painter.insert(EguiGlowPainter {
                    painter,
                    render_buffers: HashMap::new(),
                    max_texture_size: max_texture_size as usize,
                })
            }
        };

        let render_buffer = match painter.render_buffers.get_mut(&id) {
            Some(render_buffer) => render_buffer,
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
                    int_scale,
                    Transform::Flipped180, // egui glow painter wants this.
                    None,                  // TODO: Calc opaque regions?
                );

                painter.render_buffers.insert(id, texture_buffer);
                painter.render_buffers.get_mut(&id).unwrap() // ^^^
            }
        };

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect {
                min: egui::pos2(0.0, 0.0),
                max: egui::pos2(output_size.w as f32, output_size.h as f32),
            }),
            time: Some(time.as_secs_f64()),
            predicted_dt: 1.0 / 60.0,
            modifiers: convert_modifiers(self.last_modifiers),
            events: self.events.drain(..).collect(),
            hovered_files: Vec::with_capacity(0),
            dropped_files: Vec::with_capacity(0),
            focused: true,
            max_texture_side: Some(painter.max_texture_size),
            ..Default::default()
        };

        let egui::FullOutput {
            shapes,
            textures_delta,
            ..
        } = self.context.run(input.clone(), ui);

        render_buffer.render().draw(|texture| {
            renderer.bind(texture.clone())?;
            {
                let mut frame = renderer.render(output_size, Transform::Normal)?;
                frame.clear([0.; 4], &[output_geo.to_physical(int_scale)])?;
                painter.painter.paint_and_update_textures(
                    [output_size.w as u32, output_size.h as u32],
                    int_scale as f32,
                    &self.context.tessellate(shapes),
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

        Ok(FhtTextureElement(
            TextureRenderElement::from_texture_render_buffer(
                output_geo.loc.to_f64().to_physical(scale),
                &render_buffer,
                Some(alpha),
                None,
                Some(output_geo.size),
                Kind::Unspecified,
            ),
        ))
    }
}

/// An egui glow painter, based on [`egui_glow`], integrated with the smithay rendering pipewire.
pub struct EguiGlowPainter {
    painter: egui_glow::Painter,
    render_buffers: HashMap<usize, TextureRenderBuffer<GlesTexture>>,
    max_texture_size: usize,
}

impl std::fmt::Debug for EguiGlowPainter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EguiGlowPainter")
            .field("painter", &"...")
            .field("render_buffers", &self.render_buffers)
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
