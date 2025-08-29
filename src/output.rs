use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::solid::SolidColorBuffer;
use smithay::output::Output;
use smithay::reexports::calloop::RegistrationToken;
use smithay::wayland::session_lock::LockSurface;

use crate::frame_clock::FrameClock;
use crate::protocols::screencopy::ScreencopyFrame;

#[derive(Debug)]
pub struct OutputState {
    /// The state of the output in the redraw loop.
    pub redraw_state: RedrawState,
    /// The [`FrameClock`] driving this output.
    pub frame_clock: FrameClock,
    /// Whether animations are currently running on this [`Output`].
    pub animations_running: bool,
    /// The current frame sequence of the output for frame callback throttling.
    ///
    /// The issue we are trying to resolve is that we do not want to send frame callbacks at most
    /// once per refresh cycle, IE. send a frame callback every approx. one VBlank. This is
    /// required in order to avoid clients that commit but do not cause damage to keep the
    /// redraw loop going on without damage.
    ///
    /// This values gets increased when we submit buffers WITH DAMAGE, at which point we will send
    /// frames callbacks immediatly since the new client buffers have been send and hopefully
    /// presented.
    ///
    /// If we submit buffers with NO DAMAGE, we do not increase this value, but instead trigger a
    /// frame callback roughly when a VBlank should occur, at which point we will increment this
    /// value.
    pub current_frame_sequence: u32,

    /// Pending wlr_screencopy frames.
    ///
    /// How we handle wlr_screencopy is as follows:
    ///
    /// - If the client requested a screencopy **with damage**, we push the frame here and wait
    ///   until the backend draws and submits damage, by which time we render and submit the
    ///   pending screencopies.
    ///
    /// - If the client requested a screencopy **without damage**, we queue rendering of the output
    ///   to fullfill the request as soon as possible.
    pub pending_screencopies: Vec<ScreencopyFrame>,
    /// Damage tracker for [`Self::pending_screencopies`].
    pub screencopy_damage_tracker: Option<OutputDamageTracker>,

    /// Damage tracker used to draw debug damage.
    ///
    /// Lazily created when debug.draw_damage config option is enabled
    pub debug_damage_tracker: Option<OutputDamageTracker>,

    /// Lock Surface.
    ///
    /// When a client requests a session lock, it may assign a [`LockSurface`] to each output. If
    /// it does not, we instead draw a black backdrop instead of the [`LockSurface`], to ensure
    /// that user content is not displayed on the outputs.
    pub lock_surface: Option<LockSurface>,
    /// The lock backdrop of this output.
    ///
    /// This is drawn behind the lock surface (if any) to ensure that session contents are not
    /// displayed while the session is locked. If this is [`None`], a new buffer is
    /// initialized.
    pub lock_backdrop: Option<SolidColorBuffer>,
}

/// A state machine to describe where an [`Output`](smithay::output::Output) in the redraw loop.
#[derive(Debug, Default)]
pub enum RedrawState {
    /// The [`Output`](smithay::output::Output) is currently idle.
    #[default]
    Idle,
    /// A redraw has been queued for this [`Output`](smithay::output::Output) and will be
    /// fullfilled in the next dispatch cycle of compositor the event loop.
    Queued,
    /// A frame has been submitted to the DRM compositor and we are waiting for it to be presented
    /// during the next CRTC VBlank
    WaitingForVblank {
        /// Whether we need to queue redraw after the VBlank occurs.
        queued: bool,
    },
    /// We did not submit a frame to the DRM compositor and we setup a timer to send frame
    /// callbacks at the estimated next presentation time.
    WaitingForEstimatedVblankTimer {
        /// The token of the timer in the compositor event loop.
        token: RegistrationToken,
        /// Whether we need to queue redraw after the VBlank timer fires.
        queued: bool,
    },
}

impl RedrawState {
    #[inline(always)]
    pub fn is_queued(&self) -> bool {
        matches!(
            self,
            RedrawState::Queued | RedrawState::WaitingForEstimatedVblankTimer { queued: true, .. }
        )
    }

    pub fn queue(&mut self) {
        *self = match std::mem::take(self) {
            Self::Idle => Self::Queued,
            Self::WaitingForVblank { queued: false } => Self::WaitingForVblank { queued: true },
            Self::WaitingForEstimatedVblankTimer {
                token,
                queued: false,
            } => Self::WaitingForEstimatedVblankTimer {
                token,
                queued: true,
            },
            value => value, // We are already queued
        }
    }
}

use smithay::utils::{Logical, Rectangle};

/// Extension trait for an [`Output`].
pub trait OutputExt {
    /// Get this [`Output`]'s geometry in global compositor space.
    ///
    /// This uses the output's current mode size, and the advertised wl_output location.
    /// Read more at <https://wayland.app/protocols/wayland#wl_output:event:geometry>
    fn geometry(&self) -> Rectangle<i32, Logical>;
}

impl OutputExt for Output {
    fn geometry(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(self.current_location(), {
            self.current_transform()
                .transform_size(
                    self.current_mode()
                        .map(|m| m.size)
                        .unwrap_or_else(|| (0, 0).into()),
                )
                .to_f64()
                .to_logical(self.current_scale().fractional_scale())
                .to_i32_round()
        })
    }
}
