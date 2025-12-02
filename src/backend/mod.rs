use std::time::Duration;

use smithay::backend::renderer::glow::GlowRenderer;
use smithay::output::Output;

use crate::state::Fht;

#[cfg(feature = "headless-backend")]
pub mod headless;
#[cfg(feature = "udev-backend")]
pub mod udev;
#[cfg(feature = "winit-backend")]
pub mod winit;

#[allow(clippy::large_enum_variant)]
pub enum Backend {
    #[cfg(feature = "winit-backend")]
    Winit(winit::WinitData),
    #[cfg(feature = "udev-backend")]
    Udev(udev::UdevData),
    #[cfg(feature = "headless-backend")]
    Headless(headless::HeadlessData),
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

#[cfg(feature = "headless-backend")]
impl From<headless::HeadlessData> for Backend {
    fn from(value: headless::HeadlessData) -> Self {
        Self::Headless(value)
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
            Self::Winit(data) => {
                _ = output;
                _ = target_presentation_time;
                data.render(fht)
            }
            #[cfg(feature = "udev-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Udev(data) => data.render(fht, output, target_presentation_time),
            #[cfg(feature = "headless-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Headless(data) => {
                _ = target_presentation_time;
                _ = output;
                data.render(fht)
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        }
    }

    pub fn with_renderer<T>(
        &mut self,
        #[allow(unused)] f: impl FnOnce(&mut GlowRenderer) -> T,
    ) -> Option<T> {
        match self {
            #[cfg(feature = "winit-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Winit(ref mut data) => Some(f(data.renderer())),
            #[cfg(feature = "udev-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Udev(data) => {
                let mut renderer = data
                    .gpu_manager
                    .single_renderer(&data.primary_gpu)
                    .expect("No primary gpu");
                use crate::renderer::AsGlowRenderer;
                Some(f(renderer.glow_renderer_mut()))
            }
            #[allow(unreachable_patterns)]
            #[cfg(feature = "headless-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Headless(_) => {
                // No renderer, nothing todo with the closure
                let _ = f;
                None
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
            Self::Winit(_) => {
                _ = fht;
                _ = output;
                _ = mode;
                Ok(())
            }
            #[cfg(feature = "udev-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Udev(data) => data.set_output_mode(fht, output, mode),
            #[cfg(feature = "headless-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Headless(_) => {
                _ = fht;
                _ = output;
                _ = mode;
                Ok(())
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        }
    }

    pub fn update_output_vrr(
        &mut self,
        fht: &mut Fht,
        output: &Output,
        vrr: bool,
    ) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "winit-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Winit(_) => {
                _ = fht;
                _ = output;
                _ = vrr;
                Ok(())
            }
            #[cfg(feature = "udev-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Udev(data) => data.update_output_vrr(fht, output, vrr),
            #[cfg(feature = "headless-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Headless(_) => {
                _ = fht;
                _ = output;
                _ = vrr;
                Ok(())
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        }
    }

    pub fn vrr_enabled(&self, output: &Output) -> anyhow::Result<bool> {
        match self {
            #[cfg(feature = "winit-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Winit(_) => {
                _ = output;
                Ok(false)
            }
            #[cfg(feature = "udev-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Udev(data) => data.vrr_enabled(output),
            #[cfg(feature = "headless-backend")]
            #[allow(irrefutable_let_patterns)]
            Self::Headless(_) => {
                _ = output;
                Ok(false)
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        }
    }

    pub fn set_gamma(
        &mut self,
        output: &Output,
        r: Vec<u16>,
        g: Vec<u16>,
        b: Vec<u16>,
    ) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "udev-backend")]
            Self::Udev(data) => data.set_gamma(output, r, g, b),
            #[cfg(not(feature = "udev-backend"))]
            _ => unreachable!(),
            #[cfg(all(feature = "udev-backend", any(feature = "winit-backend", feature = "headless-backend")))]
            _ => unreachable!(),
        }
    }

    pub fn gamma_size(&self, output: &Output) -> Option<usize> {
        match self {
            #[cfg(feature = "udev-backend")]
            Self::Udev(data) => data.gamma_size(output).ok(),
            #[cfg(not(feature = "udev-backend"))]
            _ => None,
            #[cfg(all(feature = "udev-backend", any(feature = "winit-backend", feature = "headless-backend")))]
            _ => None,
        }
    }
}
