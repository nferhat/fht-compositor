use smithay::output::Output;
use smithay::reexports::calloop::LoopHandle;

use crate::state::State;

pub mod render;
#[cfg(feature = "udev_backend")]
pub mod udev;
#[cfg(feature = "x11_backend")]
pub mod x11;

pub enum Backend {
    None,
    #[cfg(feature = "x11_backend")]
    X11(x11::X11Data),
    #[cfg(feature = "udev_backend")]
    Udev(udev::UdevData),
}

impl Backend {
    #[cfg(feature = "x11_backend")]
    pub fn x11(&mut self) -> &mut x11::X11Data {
        if let Self::X11(data) = self {
            return data;
        }
        unreachable!("Tried to get x11 backend data on non-x11 backend!");
    }

    #[cfg(feature = "udev_backend")]
    pub fn udev(&mut self) -> &mut udev::UdevData {
        if let Self::Udev(data) = self {
            return data;
        }
        unreachable!("Tried to get udev backend data on non-udev backend!");
    }

    #[profiling::function]
    pub fn schedule_render_output(
        &mut self,
        output: &Output,
        _loop_handle: &LoopHandle<'static, State>,
    ) {
        match self {
            Self::None => panic!(),
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

pub fn init_backend_auto(state: &mut State) {
    if let Ok(backend_name) = std::env::var("FHTC_BACKEND") {
        match backend_name.trim().to_lowercase().as_str() {
            #[cfg(feature = "x11_backend")]
            "x11" => x11::init(state).unwrap(),
            #[cfg(feature = "udev_backend")]
            "kms" | "udev" => udev::init(state).unwrap(),
            x => unimplemented!("No such backend implemented!: {x}"),
        }
    }

    if std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok() {
        info!("Detected (WAYLAND_)DISPLAY. Running in nested X11 window.");
        #[cfg(feature = "x11_backend")]
        x11::init(state).unwrap();
        #[cfg(not(feature = "x11_backend"))]
        panic!("X11 backend not enabled on this build! Enable the 'x11_backend' feature when building!");
    } else {
        info!("Running from TTY, initializing Udev backend.");
        #[cfg(feature = "udev_backend")]
        udev::init(state).unwrap();
        #[cfg(not(feature = "udev_backend"))]
        panic!("Udev backend not enabled on this build! Enable the 'udev_backend' feature when building!");
    }
}
