use std::os::fd::{FromRawFd, IntoRawFd};
use std::io::Read;

use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr;
use smithay::reexports::wayland_server::{
    protocol::wl_output::WlOutput,
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
    backend::GlobalId,
};
use wayland_protocols_wlr::gamma_control::v1::server::{
    zwlr_gamma_control_manager_v1::{self, ZwlrGammaControlManagerV1},
    zwlr_gamma_control_v1::{self, ZwlrGammaControlV1},
};

use crate::state::State;

pub struct GammaControlState {
    #[allow(dead_code)] // This is used but the compiler can't see it, probably due to macros
    global: GlobalId,
}

impl GammaControlState {
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwlrGammaControlManagerV1, ()> + 'static,
    {
        let global = display.create_global::<D, ZwlrGammaControlManagerV1, _>(1, ());
        Self { global }
    }
}

impl GlobalDispatch<ZwlrGammaControlManagerV1, ()> for State {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrGammaControlManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for State {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &ZwlrGammaControlManagerV1,
        request: zwlr_gamma_control_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_gamma_control_manager_v1::Request::GetGammaControl { id, output } => {
                let wl_output = match WlOutput::from_id(_dhandle, output.id()) {
                    Ok(o) => o,
                    Err(_) => return,
                };

                if let Some(out) = Output::from_resource(&wl_output) {
                    let gamma_control = data_init.init(id, out.clone());
                    let size = state.backend.udev().gamma_size(&out).unwrap_or(0) as u32;
                    gamma_control.gamma_size(size);
                } else {
                    if let Some(out) = state.fht.space.outputs().next() {
                         data_init.init(id, out.clone());
                    }
                }
            }
            zwlr_gamma_control_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<ZwlrGammaControlV1, Output> for State {
    fn request(
        state: &mut Self,
        _client: &Client,
        gamma_control: &ZwlrGammaControlV1,
        request: zwlr_gamma_control_v1::Request,
        output: &Output,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_gamma_control_v1::Request::SetGamma { fd } => {
                let size = state.backend.udev().gamma_size(output).unwrap_or(0);
                if size == 0 {
                    gamma_control.failed();
                    return;
                }

                let expected_bytes = size * 3 * std::mem::size_of::<u16>();
                
                let mut file = unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) };
                let mut buffer = vec![0u8; expected_bytes];
                
                if file.read_exact(&mut buffer).is_err() {
                    gamma_control.failed();
                    return;
                }

                let (r_bytes, rest) = buffer.split_at(size * 2);
                let (g_bytes, b_bytes) = rest.split_at(size * 2);

                fn to_u16_vec(bytes: &[u8]) -> Vec<u16> {
                    bytes.chunks_exact(2)
                        .map(|c| u16::from_ne_bytes([c[0], c[1]]))
                        .collect()
                }

                let r = to_u16_vec(r_bytes);
                let g = to_u16_vec(g_bytes);
                let b = to_u16_vec(b_bytes);

                if let Err(err) = state.backend.udev().set_gamma(output, r, g, b) {
                    tracing::error!(?err, "Echec lors de l'application du gamma");
                    gamma_control.failed();
                }
            }
            zwlr_gamma_control_v1::Request::Destroy => {}
            _ => {}
        }
    }
}