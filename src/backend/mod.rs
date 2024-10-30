use smithay::backend::renderer::glow::GlowRenderer;
use smithay::output::Output;
use smithay::utils::{Monotonic, Time};

use crate::renderer::AsGlowRenderer;
use crate::state::Fht;

#[cfg(feature = "udev-backend")]
pub mod udev;
#[cfg(feature = "winit-backend")]
pub mod winit;

pub enum Backend {
    #[cfg(feature = "winit-backend")]
    Winit(winit::WinitData),
    #[cfg(feature = "udev-backend")]
    Udev(udev::UdevData),
}

#[cfg(feature = "winit-backend")]
impl From<winit::WinitData> for Backend {
    fn from(value: winit::WinitData) -> Self {
        Self::Winit(value)
    }
}

#[cfg(feature = "udev-backend")]
impl From<udev::UdevData> for Backend {
    fn from(value: udev::UdevData) -> Self {
        Self::Udev(value)
    }
}

impl Backend {
    #[cfg(feature = "winit-backend")]
    pub fn winit(&mut self) -> &mut winit::WinitData {
        #[allow(irrefutable_let_patterns)]
        if let Self::Winit(data) = self {
            return data;
        }
        unreachable!("Tried to get winit backend data on non-winit backend")
    }

    #[cfg(feature = "udev-backend")]
    pub fn udev(&mut self) -> &mut udev::UdevData {
        #[allow(irrefutable_let_patterns)]
        if let Self::Udev(data) = self {
            return data;
        }
        unreachable!("Tried to get udev backend data on non-udev backend")
    }

    #[profiling::function]
    pub fn render(
        &mut self,
        fht: &mut Fht,
        output: &Output,
        current_time: Time<Monotonic>,
    ) -> anyhow::Result<bool> {
        match self {
            #[cfg(feature = "winit-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Winit(data) => data.render(fht),
            #[cfg(feature = "udev-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Udev(data) => data.render(fht, output, current_time.into()),
        }
    }

    pub fn with_renderer<T>(&mut self, f: impl FnOnce(&mut GlowRenderer) -> T) -> T {
        match self {
            #[cfg(feature = "winit-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Winit(ref mut data) => f(&mut data.renderer()),
            #[cfg(feature = "udev-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Udev(data) => {
                let mut renderer = data
                    .gpu_manager
                    .single_renderer(&data.primary_gpu)
                    .expect("No primary gpu");
                f(renderer.glow_renderer_mut())
            }
        }
    }
}
