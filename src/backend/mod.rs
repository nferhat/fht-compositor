use std::time::Duration;

use smithay::backend::renderer::glow::GlowRenderer;
use smithay::output::Output;

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

    pub fn render(
        &mut self,
        #[allow(unused)] fht: &mut Fht,
        #[allow(unused)] output: &Output,
        #[allow(unused)] target_presentation_time: Duration,
    ) -> anyhow::Result<bool> {
        match self {
            #[cfg(feature = "winit-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Winit(data) => data.render(fht),
            #[cfg(feature = "udev-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Udev(data) => data.render(fht, output, target_presentation_time),
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        }
    }

    pub fn with_renderer<T>(
        &mut self,
        #[allow(unused)] f: impl FnOnce(&mut GlowRenderer) -> T,
    ) -> T {
        match self {
            #[cfg(feature = "winit-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Winit(ref mut data) => f(data.renderer()),
            #[cfg(feature = "udev-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Udev(data) => {
                let mut renderer = data
                    .gpu_manager
                    .single_renderer(&data.primary_gpu)
                    .expect("No primary gpu");
                use crate::renderer::AsGlowRenderer;
                f(renderer.glow_renderer_mut())
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        }
    }

    pub fn set_output_mode(
        &mut self,
        fht: &mut Fht,
        output: &Output,
        mode: smithay::output::Mode,
    ) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "winit-backend")]
            #[allow(irrefutable_let_patterns)]
            // winit who cares
            Self::Winit(_) => Ok(()),
            #[cfg(feature = "udev-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Udev(data) => data.set_output_mode(fht, output, mode),
        }
    }
}
