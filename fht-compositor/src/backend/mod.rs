use smithay::output::Output;
use smithay::reexports::calloop::LoopHandle;

use crate::state::State;

pub mod render;
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
    pub fn schedule_render_output(
        &mut self,
        output: &Output,
        _loop_handle: &LoopHandle<'static, State>,
    ) {
        match self {
            #[cfg(feature = "x11_backend")]
            Self::X11(ref mut data) => data.schedule_render(output),
            #[cfg(feature = "udev_backend")]
            Self::Udev(_) => {
                // TODO: Make scheduling work properly.
                // Basically the udev render loop works pretty tighly due to VBlanks, so trying
                // to render in between may or may not just lock the compositor in a state
                // where it thinks its always scheduled.

                // let _ = data.schedule_render(output, std::time::Duration::ZERO, loop_handle);
            }
        }
    }
}
