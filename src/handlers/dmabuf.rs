use smithay::delegate_dmabuf;
use smithay::wayland::dmabuf::DmabufHandler;

use crate::state::State;

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut smithay::wayland::dmabuf::DmabufState {
        &mut self.fht.dmabuf_state
    }

    #[allow(unused_mut, unreachable_code, unreachable_patterns)]
    fn dmabuf_imported(
        &mut self,
        _global: &smithay::wayland::dmabuf::DmabufGlobal,
        #[allow(unused)] dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        #[allow(unused)] notifier: smithay::wayland::dmabuf::ImportNotifier,
    ) {
        match self.backend {
            #[cfg(feature = "winit-backend")]
            #[allow(irrefutable_let_patterns)]
            crate::backend::Backend::Winit(ref mut data) => data.dmabuf_imported(&dmabuf, notifier),
            #[cfg(feature = "udev-backend")]
            #[allow(irrefutable_let_patterns)]
            crate::backend::Backend::Udev(ref mut data) => data.dmabuf_imported(dmabuf, notifier),
            _ => unreachable!(),
        };
    }
}

delegate_dmabuf!(State);
