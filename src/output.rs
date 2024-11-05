use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::output::Output;
use smithay::reexports::calloop::RegistrationToken;
use smithay::wayland::session_lock::LockSurface;

use crate::frame_clock::FrameClock;
use crate::protocols::screencopy::Screencopy;

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

    /// Pending wlr_screencopy.
    ///
    /// How the protocol works is that a client requests a screencopy frame, and then its up to the
    /// compositor to fullfill the frame request ASAP. We keep this around until we do a redraw.
    pub pending_screencopy: Option<Screencopy>,

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
    /// Whether we drew a lock backdrop on this output.
    ///
    /// For a proper session lock implementation, we draw on all outputs for at least ONE frame
    /// a black backdrop, or [`Self::lock_surface`] if any, before sending the [`lock`](lock) event
    /// to the session lock client.
    pub has_lock_backdrop: bool,
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

use smithay::utils::{Logical, Rectangle, Transform};

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
        Rectangle::from_loc_and_size(self.current_location(), {
            Transform::from(self.current_transform())
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