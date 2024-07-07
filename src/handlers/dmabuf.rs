use smithay::delegate_dmabuf;
use smithay::wayland::dmabuf::DmabufHandler;

use crate::backend::Backend;
use crate::state::State;

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut smithay::wayland::dmabuf::DmabufState {
        &mut self.fht.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &smithay::wayland::dmabuf::DmabufGlobal,
        dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: smithay::wayland::dmabuf::ImportNotifier,
    ) {
        match self.backend {
            #[cfg(feature = "x11_backend")]
            #[allow(irrefutable_let_patterns)]
            Backend::X11(ref mut data) => data.dmabuf_imported(&dmabuf, notifier),
            #[cfg(feature = "udev_backend")]
            #[allow(irrefutable_let_patterns)]
            Backend::Udev(ref mut data) => data.dmabuf_imported(dmabuf, notifier),
        };
    }
}

delegate_dmabuf!(State);
