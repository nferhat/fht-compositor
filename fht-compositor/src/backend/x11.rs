use std::time::Duration;

use anyhow::Context;
use smithay::backend::allocator::dmabuf::{Dmabuf, DmabufAllocator};
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags};
use smithay::backend::allocator::vulkan::{ImageUsageFlags, VulkanAllocator};
use smithay::backend::drm::DrmNode;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::damage::{OutputDamageTracker, RenderOutputResult};
use smithay::backend::renderer::glow::GlowRenderer;
#[cfg(feature = "egl")]
use smithay::backend::renderer::ImportEgl;
use smithay::backend::renderer::{Bind, ImportDma, ImportMemWl};
use smithay::backend::vulkan::version::Version;
use smithay::backend::vulkan::{Instance, PhysicalDevice};
use smithay::backend::x11::{Window, WindowBuilder, X11Backend, X11Event, X11Surface};
use smithay::output::{Mode, Output};
use smithay::reexports::ash::vk::ExtPhysicalDeviceDrmFn;
use smithay::reexports::calloop::ping::{make_ping, Ping};
use smithay::reexports::gbm::{self};
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::utils::{DeviceFd, Physical, Size};
use smithay::wayland::dmabuf::{
    DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, ImportNotifier,
};

use super::render::BackendAllocator;
use crate::config::CONFIG;
use crate::shell::decorations::{RoundedOutlineShader, RoundedQuadShader};
use crate::state::{Fht, State};
use crate::utils::fps::Fps;

pub struct X11Data {
    surface_pending: bool,
    surface_dirty: bool,
    surface: X11Surface,
    pub renderer: GlowRenderer,
    damage_tracker: OutputDamageTracker,
    render_ping: Ping,
    output: Output,
    fps: Fps,
    _egl_display: EGLDisplay,
    _dmabuf_global: DmabufGlobal,
    _dmabuf_default_feedback: DmabufFeedback,
    window: Window,
}

pub fn try_vulkan_allocator(drm_node: DrmNode) -> Option<VulkanAllocator> {
    Instance::new(Version::VERSION_1_2, None)
        .ok()
        .and_then(|instance| {
            PhysicalDevice::enumerate(&instance)
                .ok()
                .and_then(|devices| {
                    devices
                        .filter(|phd| phd.has_device_extension(ExtPhysicalDeviceDrmFn::name()))
                        .find(|phd| {
                            phd.primary_node().unwrap() == Some(drm_node)
                                || phd.render_node().unwrap() == Some(drm_node)
                        })
                })
        })
        .and_then(|physical_device| {
            VulkanAllocator::new(
                &physical_device,
                ImageUsageFlags::COLOR_ATTACHMENT | ImageUsageFlags::SAMPLED,
            )
            .ok()
        })
}

pub fn gbm_allocator(device: gbm::Device<DeviceFd>) -> GbmAllocator<DeviceFd> {
    GbmAllocator::new(device, GbmBufferFlags::RENDERING)
}

pub fn init(state: &mut State) -> anyhow::Result<()> {
    // Create the X11 backend and get the DRM node for direct rendering.
    let backend = X11Backend::new().context("Failed to initialize X11 backend!")?;
    let handle = backend.handle();
    let (drm_node, fd) = handle.drm_node().context("Failed to get the DRM node!")?;

    // EGL setup
    let device = gbm::Device::new(DeviceFd::from(fd)).context("Failed to create GBM device!")?;
    let egl_display =
        unsafe { EGLDisplay::new(device.clone()).context("Failed to create EGL display!") }?;
    let context = EGLContext::new(&egl_display).context("Failed to create EGL context!")?;

    // Window creation
    let window = WindowBuilder::new()
        .title("fht-compositor (X11)")
        .size((1280, 720).into())
        .build(&handle)
        .context("Failed to build X11 window!")?;
    window.set_cursor_visible(false);

    // Allocator.
    //
    // Use what the user prefers, falling back to Gbm if Vulkan failed.
    let modifiers = context.dmabuf_render_formats().iter().map(|f| f.modifier);
    let preferred_allocator = &CONFIG.renderer.allocator;
    let surface = match preferred_allocator {
        BackendAllocator::Vulkan => {
            let vulkan_allocator = try_vulkan_allocator(drm_node);
            if let Some(allocator) = vulkan_allocator {
                handle.create_surface(&window, DmabufAllocator(allocator), modifiers)
            } else {
                warn!("Failed to initialize vulkan allocator! Falling back to GBM");
                handle.create_surface(&window, DmabufAllocator(gbm_allocator(device)), modifiers)
            }
        }
        BackendAllocator::Gbm => {
            handle.create_surface(&window, DmabufAllocator(gbm_allocator(device)), modifiers)
        }
    }
    .context("Failed to create X11Surface")?;

    #[cfg_attr(not(feature = "egl"), allow(unsued_mut))]
    let mut renderer =
        unsafe { GlowRenderer::new(context) }.context("Failed to create Gles renderer!")?;
    RoundedOutlineShader::init(&mut renderer);
    RoundedQuadShader::init(&mut renderer);

    #[cfg(feature = "egl")]
    if renderer.bind_wl_display(&state.fht.display_handle).is_ok() {
        info!("EGL hardware-acceleration enabled.");
    }

    let dmabuf_formats = renderer.dmabuf_formats().collect::<Vec<_>>();
    let dmabuf_default_feedback = DmabufFeedbackBuilder::new(drm_node.dev_id(), dmabuf_formats)
        .build()
        .context("Failed to get the default dmabuf feedback!")?;
    let dmabuf_global = state
        .fht
        .dmabuf_state
        .create_global_with_default_feedback::<State>(
            &state.fht.display_handle,
            &dmabuf_default_feedback,
        );

    // Now create the output for smithay
    let size: Size<i32, Physical> = {
        let Size { w, h, .. } = window.size();
        (i32::from(w), i32::from(h)).into()
    };
    let mode = smithay::output::Mode {
        size,
        refresh: 60_000,
    };
    let output = Output::new(
        "X11-0".into(),
        smithay::output::PhysicalProperties {
            size: (0, 0).into(),
            subpixel: smithay::output::Subpixel::Unknown,
            make: "Smithay".into(),
            model: "X11Window".into(),
        },
    );
    let _output_global = output.create_global::<State>(&state.fht.display_handle);
    output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
    output.set_preferred(mode);

    // Register the output
    state.fht.add_output(output.clone());
    state.fht.focus_state.output = Some(output.clone());

    let damage_tracker = OutputDamageTracker::from_output(&output);

    let (render_ping, render_source) =
        make_ping().context("Failed to initialize render ping source!")?;
    state
        .fht
        .loop_handle
        .insert_source(render_source, |_, _, state| {
            let backend = state.backend.x11();
            if let Err(err) = backend.render_output(&mut state.fht) {
                error!(?err, "Failed to render output!");
            }
            backend.surface_dirty = false;
            backend.surface_pending = true;
        })?;
    render_ping.ping();

    let data = X11Data {
        surface_pending: true,
        surface_dirty: false,
        surface,
        renderer,
        damage_tracker,
        render_ping,
        output,
        fps: Fps::new(),
        _egl_display: egl_display,
        _dmabuf_global: dmabuf_global,
        _dmabuf_default_feedback: dmabuf_default_feedback,
        window,
    };

    state
        .fht
        .shm_state
        .update_formats(data.renderer.shm_formats());

    state
        .fht
        .loop_handle
        .insert_source(backend, move |event, _, state| {
            let backend = state.backend.x11();
            match event {
                X11Event::CloseRequested { .. } => {
                    backend.window.unmap();
                    state.fht.remove_output(&backend.output);
                    state
                        .fht
                        .stop
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                X11Event::Resized {
                    new_size: Size { w, h, .. },
                    ..
                } => {
                    let size = (i32::from(w), i32::from(h)).into();
                    let new_mode = Mode {
                        refresh: 60_000,
                        size,
                    };

                    let old_mode = backend.output.current_mode().expect("Unconfigured output!");
                    backend.output.delete_mode(old_mode);
                    backend
                        .output
                        .change_current_state(Some(new_mode), None, None, None);
                    backend.output.set_preferred(mode);
                    state.fht.output_resized(&backend.output);

                    backend.surface_dirty = true;
                    if !backend.surface_pending {
                        backend.render_ping.ping();
                    }
                }
                X11Event::Refresh { .. } | X11Event::PresentCompleted { .. } => {
                    if backend.surface_dirty {
                        backend.render_ping.ping();
                    } else {
                        backend.surface_pending = false;
                    }
                }
                X11Event::Input(event) => state.process_input_event(event),
                X11Event::Focus(_) => {}
            }
        })
        .map_err(|_| anyhow::anyhow!("Failed to insert X11 backend source to event loop!"))?;

    state.backend = super::Backend::X11(data);

    Ok(())
}

impl X11Data {
    #[profiling::function]
    pub fn schedule_render(&mut self, _: &Output) {
        self.surface_dirty = true;
        if !self.surface_pending {
            self.render_ping.ping();
        }
    }

    #[profiling::function]
    pub fn render_output(&mut self, state: &mut Fht) -> anyhow::Result<()> {
        let (buffer, buffer_age) = self
            .surface
            .buffer()
            .context("Failed to allocate buffer!")?;

        let render_elements =
            super::render::output_elements(&mut self.renderer, &self.output, state, &mut self.fps);
        self.renderer
            .bind(buffer)
            .context("Failed to bind dmabuf!")?;
        let res = self.damage_tracker.render_output(
            &mut self.renderer,
            buffer_age as usize,
            &render_elements,
            [0.1, 0.1, 0.1, 1.0],
        );
        profiling::finish_frame!();

        match res {
            Ok(RenderOutputResult { damage, states, .. }) => {
                self.surface
                    .submit()
                    .context("Failed to submit buffer to X11Surface!")?;
                self.fps.displayed();
                state.send_frames(&self.output, &states, None);
                if damage.is_some() {
                    let mut output_presentation_feedback =
                        state.take_presentation_feedback(&self.output, &states);
                    let refresh = self
                        .output
                        .current_mode()
                        .map(|mode| Duration::from_secs_f64(1_000.0 / f64::from(mode.refresh)))
                        .unwrap_or_default();
                    output_presentation_feedback.presented(
                        state.clock.now(),
                        refresh,
                        0,
                        wp_presentation_feedback::Kind::Vsync,
                    );
                }
            }
            Err(err) => {
                self.surface.reset_buffers();
                anyhow::bail!("Failed rendering! {err}");
            }
        }

        Ok(())
    }

    pub fn dmabuf_imported(&mut self, dmabuf: &Dmabuf, notifier: ImportNotifier) {
        if self.renderer.import_dmabuf(dmabuf, None).is_ok() {
            let _ = notifier.successful::<State>();
        } else {
            notifier.failed();
        }
    }
}
