use std::time::Duration;

use anyhow::Context;
use fht_animation::get_monotonic_time;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::egl::EGLDevice;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::{ImportDma, ImportEgl, ImportMemWl};
use smithay::backend::winit::{self, WinitGraphicsBackend};
use smithay::output::{Mode, Output};
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::reexports::winit::window::WindowAttributes;
use smithay::utils::Transform;
use smithay::wayland::dmabuf::{
    DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, ImportNotifier,
};

use crate::output::RedrawState;
use crate::renderer::OutputElementsResult;
use crate::state::{Fht, State};

pub struct WinitData {
    backend: WinitGraphicsBackend<GlowRenderer>,
    _backend_token: RegistrationToken,
    output: Output,
    damage_tracker: OutputDamageTracker,
    _dmabuf_state: (DmabufGlobal, Option<DmabufFeedback>),
}

impl WinitData {
    pub fn new(fht: &mut Fht) -> anyhow::Result<Self> {
        let window_attrs = WindowAttributes::default()
            .with_min_inner_size(smithay::reexports::winit::dpi::LogicalSize::new(800, 640))
            .with_max_inner_size(smithay::reexports::winit::dpi::LogicalSize::new(800, 640))
            .with_title("fht-compositor");
        let (mut backend, winit) = winit::init_from_attributes::<GlowRenderer>(window_attrs)
            .map_err(|err| anyhow::anyhow!("Failed to initialize winit backend: {err}"))?;
        let size = backend.window_size();
        crate::renderer::shaders::Shaders::init(backend.renderer());

        let token = fht
            .loop_handle
            .insert_source(winit, |event, (), state| match event {
                winit::WinitEvent::Resized { size, scale_factor } => {
                    let backend = state.backend.winit();

                    let old_mode = backend
                        .output
                        .current_mode()
                        .expect("winit output should always have a mode");
                    backend.output.delete_mode(old_mode);

                    let new_mode = Mode {
                        size,
                        refresh: 60_000,
                    };
                    backend.output.add_mode(new_mode);
                    backend.output.change_current_state(
                        Some(new_mode),
                        None,
                        Some(smithay::output::Scale::Fractional(scale_factor)),
                        None,
                    );
                    backend.output.set_preferred(new_mode);
                    state.fht.output_resized(&backend.output);
                }
                winit::WinitEvent::Input(event) => state.process_input_event(event),
                winit::WinitEvent::CloseRequested => state.fht.stop = true,
                winit::WinitEvent::Redraw => state.fht.queue_redraw(&state.backend.winit().output),
                winit::WinitEvent::Focus(_) => (), // we dont really care about focusing...
            })
            .map_err(|err| anyhow::anyhow!("Failed to insert the winit event source: {err}"))?;

        // Create a virtual output for winit
        let output = Output::new(
            String::from("winit-0"),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: String::from("winit"),
                model: String::from("window"),
            },
        );

        let mode = Mode {
            size,
            refresh: 60_000,
        };

        output.change_current_state(
            Some(mode),
            Some(Transform::Flipped180),
            Some(smithay::output::Scale::Integer(1)),
            None,
        );
        output.set_preferred(mode);
        fht.add_output(output.clone(), None);

        let render_node = EGLDevice::device_for_display(backend.renderer().egl_context().display())
            .and_then(|device| device.try_get_render_node());

        let dmabuf_default_feedback = match render_node {
            Ok(Some(node)) => {
                let dmabuf_formats = backend.renderer().dmabuf_formats();
                let dmabuf_default_feedback =
                    DmabufFeedbackBuilder::new(node.dev_id(), dmabuf_formats)
                        .build()
                        .unwrap();
                Some(dmabuf_default_feedback)
            }
            Ok(None) => {
                warn!("failed to query render node, dmabuf will use v3");
                None
            }
            Err(err) => {
                warn!(?err, "failed to egl device for display, dmabuf will use v3");
                None
            }
        };

        // if we failed to build dmabuf feedback we fall back to dmabuf v3
        // Note: egl on Mesa requires either v4 or wl_drm (initialized with bind_wl_display)
        let (dmabuf_global, dmabuf_feedback) =
            if let Some(default_feedback) = dmabuf_default_feedback {
                let dmabuf_global = fht
                    .dmabuf_state
                    .create_global_with_default_feedback::<State>(
                        &fht.display_handle,
                        &default_feedback,
                    );
                (dmabuf_global, Some(default_feedback))
            } else {
                let dmabuf_formats = backend.renderer().dmabuf_formats();
                let dmabuf_global = fht
                    .dmabuf_state
                    .create_global::<State>(&fht.display_handle, dmabuf_formats);
                (dmabuf_global, None)
            };

        fht.shm_state
            .update_formats(backend.renderer().shm_formats());

        if let Err(err) = backend.renderer().bind_wl_display(&fht.display_handle) {
            error!(?err, "Failed to enable EGL hardware acceleration");
        } else {
            info!("Enabled EGL hardware acceleration");
        };

        let damage_tracker = OutputDamageTracker::from_output(&output);

        Ok(WinitData {
            backend,
            _backend_token: token,
            damage_tracker,
            output,
            _dmabuf_state: (dmabuf_global, dmabuf_feedback),
        })
    }

    pub fn render(&mut self, fht: &mut Fht) -> anyhow::Result<bool> {
        let renderer = self.backend.renderer();
        let OutputElementsResult { elements, .. } = fht.output_elements(renderer, &self.output);

        self.backend.bind().context("Failed to bind backend")?;
        let age = self.backend.buffer_age().unwrap();
        let res = self
            .damage_tracker
            .render_output(
                self.backend.renderer(),
                age,
                &elements,
                [0.1, 0.1, 0.1, 1.0],
            )
            .unwrap();

        fht.update_primary_scanout_output(&self.output, &res.states);
        let has_damage = res.damage.is_some();
        if let Some(damage) = res.damage {
            self.backend.submit(Some(damage)).unwrap();

            let mut presentation_feedbacks =
                fht.take_presentation_feedback(&self.output, &res.states);
            let mode = self
                .output
                .current_mode()
                .expect("winit output should always have a mode");
            let refresh = Duration::from_secs_f64(1_000f64 / mode.refresh as f64);
            presentation_feedbacks.presented::<_, smithay::utils::Monotonic>(
                get_monotonic_time(),
                refresh,
                0,
                wp_presentation_feedback::Kind::empty(),
            );
        }

        let output_state = fht.output_state.get_mut(&self.output).unwrap();
        match std::mem::replace(&mut output_state.redraw_state, RedrawState::Idle) {
            RedrawState::Queued => (),
            _ => unreachable!(),
        }

        output_state.current_frame_sequence = output_state.current_frame_sequence.wrapping_add(1);

        // FIXME: this should wait until a frame callback from the host compositor, but it redraws
        // right away instead.
        if output_state.animations_running {
            self.backend.window().request_redraw();
        }

        Ok(has_damage)
    }

    pub fn dmabuf_imported(&mut self, dmabuf: &Dmabuf, notifier: ImportNotifier) {
        if self.backend.renderer().import_dmabuf(dmabuf, None).is_ok() {
            let _ = notifier.successful::<State>();
        } else {
            notifier.failed();
        }
    }

    pub fn renderer(&mut self) -> &mut GlowRenderer {
        self.backend.renderer()
    }
}
