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
use smithay::reexports::gbm;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::utils::{DeviceFd, Physical, Size};
use smithay::wayland::dmabuf::{
    DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, ImportNotifier,
};

use crate::renderer::shaders::Shaders;
use crate::state::{Fht, OutputState, RenderState, State};
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
    inner: X11Surface,
    damage_tracker: OutputDamageTracker,
    output: Output,
    fps: Fps,
    window: Window,
}

impl X11Data {
    /// Create a new instance of this x11 backend.
    ///
    /// With this backend will be create two x11 surfaces to represent two differents outputs:
    /// X11-0 and X11-1.
    ///
    /// The backend will also initialize a default dmabuf feedback, and an allocator using either
    /// Vulkan or GBM as a fallback.
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
        Shaders::init(&mut renderer);

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

                        OutputState::get(&surface.output).render_state.queue();
                    }
                    X11Event::Refresh { window_id } | X11Event::PresentCompleted { window_id } => {
                        let surface = backend.surfaces.get_mut(&window_id).unwrap();
                        OutputState::get(&surface.output).render_state.queue();
                    }
                    X11Event::Input { event, window_id } => {
                        // Adapt mouse events to match our x11 windows outputs
                        if let Some(window_id) = window_id {
                            let surface = backend.surfaces.get(&window_id).unwrap();
                            state.fht.focus_state.output = Some(surface.output.clone());
                        }
                        state.process_input_event(event)
                    }
                    X11Event::Focus {
                        focused: true,
                        window_id,
                    } => {
                        let output = backend.surfaces.get_mut(&window_id).unwrap().output.clone();
                        state.fht.focus_state.output = Some(output);
                    }
                    X11Event::Focus { focused: false, .. } => {}
                }
            })
            .map_err(|_| anyhow::anyhow!("Failed to insert X11 backend source to event loop!"))?;

        Ok(data)
    }

    /// Create a new X11 surface for this backend.
    ///
    /// This will create a new window named `fht-compositor (X11-{surface_number})`, assign a new
    /// output to this surface, with an allocator for its buffers.
    pub fn new_surface(&mut self, state: &mut Fht) -> anyhow::Result<()> {
        let window_idx = self.surfaces.len();
        let window = WindowBuilder::new()
            .size((680, 480).into())
            .title(&format!("fht-compositor (X11-{window_idx})"))
            .build(&self.backend_handle)
            .context("Failed to build X11 window!")?;

        // Try vulkan, otherwise try GBM.
        let device = self.gbm_device.clone();
        let context = self.renderer.egl_context();
        let modifiers = context.dmabuf_render_formats().iter().map(|f| f.modifier);
        let surface = {
            let vulkan_allocator = Instance::new(Version::VERSION_1_2, None)
                .ok()
                .and_then(|instance| {
                    PhysicalDevice::enumerate(&instance)
                        .ok()
                        .and_then(|devices| {
                            devices
                                .filter(|phd| {
                                    phd.has_device_extension(ExtPhysicalDeviceDrmFn::name())
                                })
                                .find(|phd| {
                                    phd.primary_node().unwrap() == Some(self.drm_node)
                                        || phd.render_node().unwrap() == Some(self.drm_node)
                                })
                        })
                })
                .and_then(|physical_device| {
                    VulkanAllocator::new(
                        &physical_device,
                        ImageUsageFlags::COLOR_ATTACHMENT | ImageUsageFlags::SAMPLED,
                    )
                    .ok()
                });
            if let Some(allocator) = vulkan_allocator {
                self.backend_handle
                    .create_surface(&window, DmabufAllocator(allocator), modifiers)
            } else {
                warn!("Failed to initialize vulkan allocator! Falling back to GBM");
                self.backend_handle.create_surface(
                    &window,
                    DmabufAllocator(GbmAllocator::new(device, GbmBufferFlags::RENDERING)),
                    modifiers,
                )
            }
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
        // Create rendering state
        let damage_tracker = OutputDamageTracker::from_output(&output);
        OutputState::get(&output).render_state.queue();

        self.surfaces.insert(
            window.id(),
            Surface {
                inner: surface,
                damage_tracker,
                output,
                fps: Fps::new(),
                window,
            },
        );

        Ok(())
    }

    /// Render a given [`Output`], if an associated [`Surface`] is found for it.
    #[profiling::function]
    pub fn render(
        &mut self,
        state: &mut Fht,
        output: &Output,
        current_time: Duration,
    ) -> anyhow::Result<bool> {
        let Some(surface) = self.surfaces.values_mut().find(|s| s.output == *output) else {
            anyhow::bail!("Tried to render a non existing surface!");
        };

        surface.fps.start();
        let (buffer, buffer_age) = surface
            .inner
            .buffer()
            .context("Failed to allocate buffer!")?;

        let output_elements_result =
            state.output_elements(&mut self.renderer, &surface.output, &mut surface.fps);
        surface.fps.elements();

        self.renderer
            .bind(buffer)
            .context("Failed to bind dmabuf!")?;
        let res = surface.damage_tracker.render_output(
            &mut self.renderer,
            buffer_age as usize,
            &output_elements_result.render_elements,
            [0.1, 0.1, 0.1, 1.0],
        );

        surface.fps.render();
        profiling::finish_frame!();

        match res {
            Ok(RenderOutputResult { damage, states, .. }) => {
                let mut output_state = OutputState::get(&surface.output);
                match std::mem::take(&mut output_state.render_state) {
                    RenderState::Queued => (),
                    _ => unreachable!(),
                }
                output_state.current_frame_sequence =
                    output_state.current_frame_sequence.wrapping_add(1);

                state.update_primary_scanout_output(output, &states);

                surface
                    .inner
                    .submit()
                    .context("Failed to submit buffer to X11Surface!")?;
                surface.fps.displayed();
                if damage.is_some() {
                    let mut output_presentation_feedback =
                        state.take_presentation_feedback(&surface.output, &states);
                    let refresh = surface
                        .output
                        .current_mode()
                        .map(|mode| Duration::from_secs_f64(1_000.0 / f64::from(mode.refresh)))
                        .unwrap_or_default();
                    output_presentation_feedback.presented::<_, smithay::utils::Monotonic>(
                        current_time,
                        refresh,
                        0,
                        wp_presentation_feedback::Kind::Vsync,
                    );

                    // We damaged so render after
                    output_state.render_state.queue();

                    // Also render for screencopy
                    #[cfg(feature = "xdg-screencast-portal")]
                    {
                        drop(output_state);
                        state.render_screencast(
                            output,
                            &mut self.renderer,
                            &output_elements_result,
                        );
                        surface.fps.screencast();
                    }

                    Ok(true)
                } else {
                    // Didn't render anything, no need to schedule or something since we are
                    // already managed by a parent compositor(even xwayland) to send frames.
                    Ok(false)
                }
            }
            Err(err) => {
                surface.inner.reset_buffers();
                anyhow::bail!("Failed rendering! {err}");
            }
        }
    }

    /// Import a [`Dmabuf`] to this renderer.
    pub fn dmabuf_imported(&mut self, dmabuf: &Dmabuf, notifier: ImportNotifier) {
        if self.renderer.import_dmabuf(dmabuf, None).is_ok() {
            let _ = notifier.successful::<State>();
        } else {
            notifier.failed();
        }
    }
}
