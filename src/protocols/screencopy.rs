use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use smithay::backend::allocator::{dmabuf::Dmabuf, Buffer};
use smithay::backend::renderer::{buffer_type, BufferType};
use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::{
    self, ZwlrScreencopyFrameV1,
};
use smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::{
    self, ZwlrScreencopyManagerV1,
};
use smithay::reexports::wayland_server;
use smithay::reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm};
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, Resource};
use smithay::utils::{Physical, Point, Rectangle};
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::shm::{self, shm_format_to_fourcc};
use tracing::trace;

const VERSION: u32 = 3;

pub struct ScreencopyManagerState;

pub struct ScreencopyManagerGlobalData {
    filter: Box<dyn Fn(&Client) -> bool + Send + Sync>,
}

impl ScreencopyManagerState {
    pub fn new<D, F>(display: &DisplayHandle, filter: F) -> Self
    where
        D: GlobalDispatch<ZwlrScreencopyManagerV1, ScreencopyManagerGlobalData>
            + Dispatch<ZwlrScreencopyManagerV1, ()>
            + Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState>
            + ScreencopyHandler
            + 'static,
        F: Fn(&Client) -> bool + Send + Sync + 'static,
    {
        let global_data = ScreencopyManagerGlobalData {
            filter: Box::new(filter),
        };
        display.create_global::<D, ZwlrScreencopyManagerV1, _>(VERSION, global_data);
        Self
    }
}

impl<D> GlobalDispatch<ZwlrScreencopyManagerV1, ScreencopyManagerGlobalData, D>
    for ScreencopyManagerState
where
    D: GlobalDispatch<ZwlrScreencopyManagerV1, ScreencopyManagerGlobalData>
        + Dispatch<ZwlrScreencopyManagerV1, ()>
        + Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState>
        + ScreencopyHandler
        + 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: wayland_server::New<ZwlrScreencopyManagerV1>,
        _global_data: &ScreencopyManagerGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global_data: &ScreencopyManagerGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<ZwlrScreencopyManagerV1, (), D> for ScreencopyManagerState
where
    D: GlobalDispatch<ZwlrScreencopyManagerV1, ScreencopyManagerGlobalData>
        + Dispatch<ZwlrScreencopyManagerV1, ()>
        + Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState>
        + ScreencopyHandler
        + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        manager: &ZwlrScreencopyManagerV1,
        request: <ZwlrScreencopyManagerV1 as wayland_server::Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        let (frame, overlay_cursor, physical_region, output) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => {
                let Some(output) = Output::from_resource(&output) else {
                    trace!("Screencopy client requested invalid output");
                    let frame = data_init.init(frame, ScreencopyFrameState::Failed);
                    frame.failed();
                    return;
                };

                let Some(physical_size) = output.current_mode().map(|mode| mode.size) else {
                    trace!("Screencopy output has no mode!");
                    let frame = data_init.init(frame, ScreencopyFrameState::Failed);
                    frame.failed();
                    return;
                };

                (
                    frame,
                    overlay_cursor,
                    Rectangle::from_loc_and_size(Point::default(), physical_size),
                    output,
                )
            }
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                // NOTE: the given coordinates are in transformed output coordinate space.
                x,
                y,
                width,
                height,
            } => {
                if width <= 0 || height <= 0 {
                    trace!("Screencopy client requested region with negative size");
                    let frame = data_init.init(frame, ScreencopyFrameState::Failed);
                    frame.failed();
                    return;
                }

                let Some(output) = Output::from_resource(&output) else {
                    trace!("Screencopy client requested invalid output");
                    let frame = data_init.init(frame, ScreencopyFrameState::Failed);
                    frame.failed();
                    return;
                };

                let Some(physical_size) = output.current_mode().map(|mode| mode.size) else {
                    trace!("Screencopy output has no mode!");
                    let frame = data_init.init(frame, ScreencopyFrameState::Failed);
                    frame.failed();
                    return;
                };

                let transform = output.current_transform();
                let transformed_rect =
                    Rectangle::from_loc_and_size((0, 0), transform.transform_size(physical_size));
                // Now clamp the screencopy region inside the output space
                let screencopy_region = Rectangle::from_loc_and_size((x, y), (width, height));
                let output_scale = output.current_scale().fractional_scale();
                let physical_rect = screencopy_region.to_physical_precise_round(output_scale);
                let Some(clamped_rect) = physical_rect.intersection(transformed_rect) else {
                    trace!("Screencopy client requested region outside of output");
                    let frame = data_init.init(frame, ScreencopyFrameState::Failed);
                    frame.failed();
                    return;
                };

                // Untransform the region to the actual physical rect
                let untransformed_region = transform
                    .invert()
                    .transform_rect_in(clamped_rect, &transformed_rect.size);

                (frame, overlay_cursor, untransformed_region, output)
            }
            zwlr_screencopy_manager_v1::Request::Destroy => return,
            _ => unreachable!(),
        };

        // Create the frame.
        let info = ScreencopyFrameInfo {
            output,
            overlay_cursor: overlay_cursor != 0,
            physical_region,
        };
        let frame = data_init.init(
            frame,
            ScreencopyFrameState::Pending {
                info,
                copied: Arc::new(AtomicBool::new(false)),
            },
        );

        let buffer_size = physical_region.size;

        // Send desired SHM buffer parameters.
        frame.buffer(
            wl_shm::Format::Xrgb8888,
            buffer_size.w as u32,
            buffer_size.h as u32,
            buffer_size.w as u32 * 4,
        );

        if manager.version() >= 3 {
            // Send desired DMA buffer parameters.
            frame.linux_dmabuf(
                smithay::backend::allocator::Fourcc::Xrgb8888 as u32,
                buffer_size.w as u32,
                buffer_size.h as u32,
            );

            // Notify client that all supported buffers were enumerated.
            frame.buffer_done();
        }
    }
}

pub trait ScreencopyHandler {
    /// A client has requested a new [`ScreencopyFrame`].
    ///
    /// The compositor must fullfill the request as soon as possible depending on frame parameters.
    fn new_frame(&mut self, frame: ScreencopyFrame);
}

#[allow(missing_docs)]
#[macro_export]
macro_rules! delegate_screencopy {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1: $crate::protocols::screencopy::ScreencopyManagerGlobalData
        ] => $crate::protocols::screencopy::ScreencopyManagerState);

        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1: ()
        ] => $crate::protocols::screencopy::ScreencopyManagerState);

        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1: $crate::protocols::screencopy::ScreencopyFrameState
        ] => $crate::protocols::screencopy::ScreencopyManagerState);
    };
}

/// Information associated with a [`ScreencopyFrame`].
/// This structure gets created from [`Request::CaptureOutput`] or [`Request::CaptureOutputRegion`].
#[derive(Clone, Debug)]
pub struct ScreencopyFrameInfo {
    /// The output we are screencopying from.
    output: Output,
    /// The physical region of the output we are screencopying from.
    physical_region: Rectangle<i32, Physical>,
    /// Whether we should include the cursor in the screencopy.
    overlay_cursor: bool,
}

/// A global state of a [`ScreencopyFrame`].
pub enum ScreencopyFrameState {
    /// We failed to initialize the [`ScreencopyFrame`]
    Failed,
    /// The [`ScreencopyFrame`] is pending and awaiting requests.
    Pending {
        info: ScreencopyFrameInfo,
        copied: Arc<AtomicBool>,
    },
}

/// A buffer attached to a [`ScreencopyFrame`].
///
/// It is provided by the client for the compositor to render into.
#[derive(Clone, Debug)]
pub enum ScreencopyBuffer {
    /// The client requested a [`wl_shm`]-based buffer.
    Shm(WlBuffer),
    /// The client requested a [`dmabuf`]-based buffer.
    Dma(Dmabuf),
}

impl<D> Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState, D> for ScreencopyManagerState
where
    D: Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState> + ScreencopyHandler + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: <ZwlrScreencopyFrameV1 as wayland_server::Resource>::Request,
        data: &ScreencopyFrameState,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        if matches!(request, zwlr_screencopy_frame_v1::Request::Destroy) {
            return;
        }

        let (info, copied) = match data {
            ScreencopyFrameState::Failed => return,
            ScreencopyFrameState::Pending { info, copied } => (info, copied),
        };

        if copied.load(Ordering::SeqCst) {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                "copy was already requested",
            );
            return;
        }

        let (buffer, with_damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            _ => unreachable!(),
        };

        let buffer = match buffer_type(&buffer) {
            Some(BufferType::Shm) => {
                if !shm::with_buffer_contents(&buffer, |_buf, shm_len, buffer_data| {
                    buffer_data.format == wl_shm::Format::Xrgb8888
                        && buffer_data.stride == info.physical_region.size.w * 4
                        && buffer_data.height == info.physical_region.size.h
                        && shm_len as i32 == buffer_data.stride * buffer_data.height
                })
                .unwrap_or(false)
                {
                    frame.post_error(
                        zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                        "invalid buffer",
                    );
                    return;
                }

                ScreencopyBuffer::Shm(buffer)
            }
            Some(BufferType::Dma) => {
                let dmabuf = get_dmabuf(&buffer).unwrap();
                if !(Some(dmabuf.format().code) == shm_format_to_fourcc(wl_shm::Format::Xrgb8888)
                    && dmabuf.width() == info.physical_region.size.w as u32
                    && dmabuf.height() == info.physical_region.size.h as u32)
                {
                    frame.post_error(
                        zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                        "invalid buffer",
                    );
                    return;
                }

                ScreencopyBuffer::Dma(dmabuf.clone())
            }
            _ => {
                frame.post_error(
                    zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                    "invalid buffer",
                );
                return;
            }
        };

        copied.store(true, Ordering::SeqCst);

        state.new_frame(ScreencopyFrame {
            with_damage,
            buffer,
            frame: frame.clone(),
            info: info.clone(),
            submitted: false,
        });
    }
}

/// An instance of a [`ZwlrScreencopyFrameV1`].
#[derive(Debug)]
pub struct ScreencopyFrame {
    /// The information associated with this frame on creation.
    info: ScreencopyFrameInfo,
    /// The protocol frame object.
    frame: ZwlrScreencopyFrameV1,
    /// Whether the client requested to render this frame only on damage.
    ///
    /// If this is true, the compositor must wait until there's damage on the output to render into
    /// [`Self::buffer`] and call [`Self::submit`].
    with_damage: bool,
    /// The buffer provided by the client the compositor should render into.
    buffer: ScreencopyBuffer,
    /// Whether we successfully submitted this frame.
    submitted: bool,
}

impl Drop for ScreencopyFrame {
    fn drop(&mut self) {
        if !self.submitted {
            self.frame.failed();
        }
    }
}

#[allow(unused)] // TODO: Make overlay cursor work.
impl ScreencopyFrame {
    /// The output to screencopy from.
    pub fn output(&self) -> &Output {
        &self.info.output
    }

    /// The buffer provided by the client for this [`ScreencopyFrame`].
    pub fn buffer(&self) -> &ScreencopyBuffer {
        &self.buffer
    }

    /// The physical region the client asked to capture.
    pub fn physical_region(&self) -> Rectangle<i32, Physical> {
        self.info.physical_region
    }

    /// Whether we should include the cursor when screencopying.
    pub fn overlay_cursor(&self) -> bool {
        self.info.overlay_cursor
    }

    /// Whether the client requested to submit only on damage.
    pub fn with_damage(&self) -> bool {
        self.with_damage
    }

    /// Submit damage to this [`ScreencopyFrame`].
    pub fn damage(&mut self, damage: &[Rectangle<i32, Physical>]) {
        if !self.with_damage {
            return;
        }

        for Rectangle { loc, size } in damage {
            self.frame
                .damage(loc.x as u32, loc.y as u32, size.w as u32, size.h as u32);
        }
    }

    /// Mark this frame as failed.
    ///
    /// This function consumes the [`ScreencopyFrame`], as per the protocol, we should not use the
    /// frame object after submitting, since the client will delete it.
    pub fn failed(self) {}

    /// Mark this frame as submitted.
    ///
    /// This function consumes the [`ScreencopyFrame`], as per the protocol, we should not use the
    /// frame object after submitting, since the client will delete it.
    pub fn submit(mut self, y_invert: bool, time: Duration) {
        // Notify client that buffer is ordinary.
        self.frame.flags(if y_invert {
            zwlr_screencopy_frame_v1::Flags::YInvert
        } else {
            zwlr_screencopy_frame_v1::Flags::empty()
        });

        // Notify client about successful copy.
        let tv_sec_hi = (time.as_secs() >> 32) as u32;
        let tv_sec_lo = (time.as_secs() & 0xFFFFFFFF) as u32;
        let tv_nsec = time.subsec_nanos();
        self.frame.ready(tv_sec_hi, tv_sec_lo, tv_nsec);

        // Mark frame as submitted to ensure destructor isn't run.
        self.submitted = true;
    }
}
