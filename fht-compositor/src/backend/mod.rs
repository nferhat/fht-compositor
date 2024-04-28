use smithay::backend::renderer::glow::GlowRenderer;
use smithay::output::Output;
use smithay::utils::{Monotonic, Time};

use crate::renderer::AsGlowRenderer;
use crate::state::Fht;

#[cfg(feature = "udev_backend")]
pub mod udev;
#[cfg(feature = "x11_backend")]
pub mod x11;

pub enum Backend {
    #[cfg(feature = "x11_backend")]
    X11(x11::X11Data),
    #[cfg(feature = "udev_backend")]
    Udev(udev::UdevData),
}

#[cfg(feature = "x11_backend")]
impl From<x11::X11Data> for Backend {
    fn from(value: x11::X11Data) -> Self {
        Self::X11(value)
    }
}

#[cfg(feature = "udev_backend")]
impl From<udev::UdevData> for Backend {
    fn from(value: udev::UdevData) -> Self {
        Self::Udev(value)
    }
}

impl Backend {
    /// Access the underlying X11 backend, if any.
    ///
    /// # PANICS
    ///
    /// This panics if the current backend is not X11.
    #[cfg(feature = "x11_backend")]
    pub fn x11(&mut self) -> &mut x11::X11Data {
        if let Self::X11(data) = self {
            return data;
        }
        unreachable!("Tried to get x11 backend data on non-x11 backend!");
    }

    /// Access the underlying udev backend, if any.
    ///
    /// # PANICS
    ///
    /// This panics if the current backend is not udev.
    #[cfg(feature = "udev_backend")]
    pub fn udev(&mut self) -> &mut udev::UdevData {
        if let Self::Udev(data) = self {
            return data;
        }
        unreachable!("Tried to get udev backend data on non-udev backend!");
    }

    /// Request the backend to schedule a next frame for this output.
    ///
    /// The backend is free to oblige or discard your request, based on internal state like Vblank
    /// state, or if a frame has already been scheduled.
    #[profiling::function]
    pub fn render(
        &mut self,
        fht: &mut Fht,
        output: &Output,
        current_time: Time<Monotonic>,
    ) -> anyhow::Result<bool> {
        match self {
            #[cfg(feature = "x11_backend")]
            Self::X11(ref mut data) => data.render(fht, output, current_time.into()),
            #[cfg(feature = "udev_backend")]
            Self::Udev(data) => data.render(fht, output, current_time.into()),
        }
    }

    /// Get the primary renderer of the backend
    pub fn with_renderer<T, F: FnOnce(&mut GlowRenderer) -> T>(&mut self, f: F) -> T {
        match self {
            #[cfg(feature = "x11_backend")]
            Self::X11(ref mut data) => f(&mut data.renderer),
            #[cfg(feature = "udev_backend")]
            Self::Udev(ref mut data) => {
                let mut multi_renderer = data.gpu_manager.single_renderer(&data.primary_gpu).unwrap();
                f(multi_renderer.glow_renderer_mut())
            }
        }
    }
}
