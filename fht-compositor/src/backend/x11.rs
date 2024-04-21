use std::collections::HashMap;
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
use smithay::backend::x11::{Window, WindowBuilder, X11Backend, X11Event, X11Handle, X11Surface};
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
    pub renderer: GlowRenderer,
    // u32 is the XID of the surface's window
    pub surfaces: HashMap<u32, Surface>,
    backend_handle: X11Handle,
    _egl_display: EGLDisplay,
    gbm_device: gbm::Device<DeviceFd>,
    drm_node: DrmNode,
    _dmabuf_global: DmabufGlobal,
    _dmabuf_default_feedback: DmabufFeedback,
}

pub struct Surface {
    pending: bool,
    dirty: bool,
    inner: X11Surface,
    damage_tracker: OutputDamageTracker,
    render_ping: Ping,
    output: Output,
    fps: Fps,
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

impl X11Data {
    pub fn new(state: &mut Fht) -> anyhow::Result<Self> {
        // Create the X11 backend and get the DRM node for direct rendering.
        let backend = X11Backend::new().context("Failed to initialize X11 backend!")?;
        let handle = backend.handle();
        let (drm_node, fd) = handle.drm_node().context("Failed to get the DRM node!")?;

        // EGL setup
        let device =
            gbm::Device::new(DeviceFd::from(fd)).context("Failed to create GBM device!")?;
        let egl_display =
            unsafe { EGLDisplay::new(device.clone()).context("Failed to create EGL display!") }?;
        let context = EGLContext::new(&egl_display).context("Failed to create EGL context!")?;

        #[cfg_attr(not(feature = "egl"), allow(unsued_mut))]
        let mut renderer =
            unsafe { GlowRenderer::new(context) }.context("Failed to create Gles renderer!")?;
        RoundedOutlineShader::init(&mut renderer);
        RoundedQuadShader::init(&mut renderer);

        #[cfg(feature = "egl")]
        if renderer.bind_wl_display(&state.display_handle).is_ok() {
            info!("EGL hardware-acceleration enabled.");
        }

        let dmabuf_formats = renderer.dmabuf_formats().collect::<Vec<_>>();
        let dmabuf_default_feedback = DmabufFeedbackBuilder::new(drm_node.dev_id(), dmabuf_formats)
            .build()
            .context("Failed to get the default dmabuf feedback!")?;
        let dmabuf_global = state
            .dmabuf_state
            .create_global_with_default_feedback::<State>(
                &state.display_handle,
                &dmabuf_default_feedback,
            );

        state.shm_state.update_formats(renderer.shm_formats());
        let backend_handle = backend.handle();

        let mut data = X11Data {
            renderer,
            surfaces: HashMap::new(),
            backend_handle,
            _egl_display: egl_display,
            drm_node,
            gbm_device: device,
            _dmabuf_global: dmabuf_global,
            _dmabuf_default_feedback: dmabuf_default_feedback,
        };

        // We create 2 x11 windows to simulate two different outputs.
        data.new_surface(state)
            .expect("Failed to create x11 surfaces");
        data.new_surface(state)
            .expect("Failed to create 2nd x11 surfaces");

        state
            .loop_handle
            .insert_source(backend, move |event, _, state| {
                let backend = state.backend.x11();
                match event {
                    X11Event::CloseRequested { window_id, .. } => {
                        let surface = backend.surfaces.remove(&window_id).unwrap();
                        surface.window.unmap();
                        state.fht.remove_output(&surface.output);
                    }
                    X11Event::Resized {
                        new_size: Size { w, h, .. },
                        window_id,
                        ..
                    } => {
                        let surface = backend.surfaces.get_mut(&window_id).unwrap();

                        let size = (i32::from(w), i32::from(h)).into();
                        let new_mode = Mode {
                            refresh: 60_000,
                            size,
                        };

                        let old_mode = surface.output.current_mode().expect("Unconfiguredoutput!");
                        surface.output.delete_mode(old_mode);
                        surface
                            .output
                            .change_current_state(Some(new_mode), None, None, None);
                        surface.output.set_preferred(new_mode);
                        state.fht.output_resized(&surface.output);

                        surface.dirty = true;
                        if !surface.pending {
                            surface.render_ping.ping();
                        }
                    }
                    X11Event::Refresh { window_id } | X11Event::PresentCompleted { window_id } => {
                        let surface = backend.surfaces.get_mut(&window_id).unwrap();

                        if surface.dirty {
                            surface.render_ping.ping();
                        } else {
                            surface.pending = false;
                        }
                    }
                    X11Event::Input {event, window_id} => {
                        // Adapt mouse events to match our x11 windows outputs
                        if let Some(window_id) = window_id {
                            let surface = backend.surfaces.get(&window_id).unwrap();
                            state.fht.focus_state.output = Some(surface.output.clone());
                        }
                        state.process_input_event(event)
                    },
                    X11Event::Focus(_) => {}
                }
            })
            .map_err(|_| anyhow::anyhow!("Failed to insert X11 backend source to event loop!"))?;

        Ok(data)
    }

    pub fn new_surface(&mut self, state: &mut Fht) -> anyhow::Result<()> {
        let window_idx = self.surfaces.len();
        let window = WindowBuilder::new()
            .size((680, 480).into())
            .title(&format!("fht-compositor (X11-{window_idx})"))
            .build(&self.backend_handle)
            .context("Failed to build X11 window!")?;

        // Allocator.
        //
        // Use what the user prefers, falling back to Gbm if Vulkan failed.
        let device = self.gbm_device.clone();
        let context = self.renderer.egl_context();
        let modifiers = context.dmabuf_render_formats().iter().map(|f| f.modifier);
        let preferred_allocator = &CONFIG.renderer.allocator;
        let surface = match preferred_allocator {
            BackendAllocator::Vulkan => {
                let vulkan_allocator = try_vulkan_allocator(self.drm_node);
                if let Some(allocator) = vulkan_allocator {
                    self.backend_handle.create_surface(
                        &window,
                        DmabufAllocator(allocator),
                        modifiers,
                    )
                } else {
                    warn!("Failed to initialize vulkan allocator! Falling back to GBM");
                    self.backend_handle.create_surface(
                        &window,
                        DmabufAllocator(gbm_allocator(device)),
                        modifiers,
                    )
                }
            }
            BackendAllocator::Gbm => self.backend_handle.create_surface(
                &window,
                DmabufAllocator(gbm_allocator(device)),
                modifiers,
            ),
        }
        .context("Failed to create X11Surface")?;

        let size: Size<i32, Physical> = {
            let Size { w, h, .. } = window.size();
            (i32::from(w), i32::from(h)).into()
        };
        let mode = smithay::output::Mode {
            size,
            refresh: 60_000,
        };
        let output = Output::new(
            format!("X11-{window_idx}"),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: "Smithay".into(),
                model: "X11Window".into(),
            },
        );
        let _output_global = output.create_global::<State>(&state.display_handle);
        output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
        output.set_preferred(mode);

        // Register the output
        state.add_output(output.clone());
        state.focus_state.output = Some(output.clone());

        let damage_tracker = OutputDamageTracker::from_output(&output);

        let (render_ping, render_source) =
            make_ping().context("Failed to initialize render ping source!")?;

        self.surfaces.insert(
            window.id(),
            Surface {
                pending: false,
                dirty: true,
                inner: surface,
                damage_tracker,
                render_ping: render_ping.clone(),
                output: output.clone(),
                fps: Fps::new(),
                window,
            },
        );

        state
            .loop_handle
            .insert_source(render_source, move |_, _, state| {
                let backend = state.backend.x11();
                if let Err(err) = backend.render_output(&mut state.fht, &output) {
                    error!(?err, "Failed to render output!");
                }
            })?;
        render_ping.ping();

        Ok(())
    }

    #[profiling::function]
    pub fn schedule_render(&mut self, output: &Output) {
        let Some(surface) = self.surfaces.values_mut().find(|s| s.output == *output) else {
            error!("Tried to render a non existing surface!");
            return;
        };
        surface.dirty = true;
        if !surface.pending {
            surface.render_ping.ping();
        }
    }

    #[profiling::function]
    pub fn render_output(&mut self, state: &mut Fht, output: &Output) -> anyhow::Result<()> {
        let Some(surface) = self.surfaces.values_mut().find(|s| s.output == *output) else {
            error!("Tried to render a non existing surface!");
            return Ok(());
        };

        state.advance_animations(&surface.output, state.clock.now().into());
        surface.fps.start();

        let (buffer, buffer_age) = surface
            .inner
            .buffer()
            .context("Failed to allocate buffer!")?;

        let render_elements = super::render::output_elements(
            &mut self.renderer,
            &surface.output,
            state,
            &mut surface.fps,
        );
        surface.fps.elements();

        self.renderer
            .bind(buffer)
            .context("Failed to bind dmabuf!")?;
        let res = surface.damage_tracker.render_output(
            &mut self.renderer,
            buffer_age as usize,
            &render_elements,
            [0.1, 0.1, 0.1, 1.0],
        );

        surface.fps.render();
        profiling::finish_frame!();

        match res {
            Ok(RenderOutputResult { damage, states, .. }) => {
                surface
                    .inner
                    .submit()
                    .context("Failed to submit buffer to X11Surface!")?;
                surface.fps.displayed();
                state.send_frames(&surface.output, &states, None);
                if damage.is_some() {
                    let mut output_presentation_feedback =
                        state.take_presentation_feedback(&surface.output, &states);
                    let refresh = surface
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
                surface.inner.reset_buffers();
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
