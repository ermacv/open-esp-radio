//! Beacon completion owns its DTIM boundary, independently of the mutable
//! template and of group frames buffered after preparation.

use core::num::NonZeroU16;

/// One beacon's bytes and its terminal continuation. The runtime moves the
/// context into the same admitted owner as the encoded MPDU and ticket.
/// Its outer BSS/installation owner must finish outstanding publications
/// before returning a restart bank.
#[must_use = "publish the beacon with its context or complete it as unsent"]
#[derive(Debug)]
pub struct ApBeaconPublication<'frame> {
    frame: &'frame mut [u8],
    context: ApBeaconTxContext,
}

impl<'frame> ApBeaconPublication<'frame> {
    pub(super) fn new(frame: &'frame mut [u8], advertised_frames: u16) -> Self {
        Self {
            frame,
            context: ApBeaconTxContext { advertised_frames },
        }
    }

    /// Read the prepared MPDU without detaching its terminal context.
    pub fn frame(&self) -> &[u8] {
        self.frame
    }

    /// Move the context alongside the frame's port admission. A preparation
    /// or admission refusal completes that context with `Unsent`.
    pub fn into_parts(self) -> (&'frame mut [u8], ApBeaconTxContext) {
        (self.frame, self.context)
    }
}

/// Hardware evidence for one beacon. Beacons are group addressed: Delivered
/// means confirmed on-air publication, never a fabricated peer ACK.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApBeaconTxOutcome {
    Delivered,
    NotDelivered,
    Indeterminate,
    Withdrawn,
    Unsent,
}

/// The affine continuation of one concrete beacon. It cannot be rebuilt from
/// the current TIM, a TBTT event or the current group queue length.
///
/// ```compile_fail,E0382
/// use oer_ieee80211_ap::beacon::{ApBeaconTxContext, ApBeaconTxOutcome};
/// fn twice(context: ApBeaconTxContext) {
///     let first = context.complete(ApBeaconTxOutcome::Delivered);
///     let second = context.complete(ApBeaconTxOutcome::Delivered);
/// }
/// ```
///
/// ```
/// use oer_ieee80211_ap::beacon::{ApBeaconTxContext, ApBeaconTxOutcome, ApDtimRelease};
/// fn once(context: ApBeaconTxContext) -> Option<ApDtimRelease> {
///     context.complete(ApBeaconTxOutcome::Delivered)
/// }
/// ```
#[must_use = "complete the context at this beacon's terminal"]
#[derive(Debug)]
pub struct ApBeaconTxContext {
    advertised_frames: u16,
}

impl ApBeaconTxContext {
    /// Only success grants release of the captured prefix. On every other
    /// terminal the group payloads stay buffered for a later DTIM.
    pub fn complete(self, outcome: ApBeaconTxOutcome) -> Option<ApDtimRelease> {
        match outcome {
            ApBeaconTxOutcome::Delivered => {
                NonZeroU16::new(self.advertised_frames).map(|frames| ApDtimRelease { frames })
            }
            ApBeaconTxOutcome::NotDelivered
            | ApBeaconTxOutcome::Indeterminate
            | ApBeaconTxOutcome::Withdrawn
            | ApBeaconTxOutcome::Unsent => None,
        }
    }
}

/// A nonempty prefix advertised by a successfully published DTIM beacon.
/// Transferring it to the caller's retained group queue consumes the grant.
///
/// ```compile_fail,E0382
/// use oer_ieee80211_ap::beacon::ApDtimRelease;
/// fn twice(grant: ApDtimRelease) {
///     let first = grant.into_frame_count();
///     let second = grant.into_frame_count();
/// }
/// ```
///
/// ```
/// use oer_ieee80211_ap::beacon::ApDtimRelease;
/// fn once(grant: ApDtimRelease) -> core::num::NonZeroU16 {
///     grant.into_frame_count()
/// }
/// ```
#[must_use = "release this prefix or retain its payloads for a later DTIM"]
#[derive(Debug, Eq, PartialEq)]
pub struct ApDtimRelease {
    frames: NonZeroU16,
}

impl ApDtimRelease {
    /// Transfer the exact prefix length into the caller's queue release
    /// operation. Frames appended after preparation are outside this grant.
    pub const fn into_frame_count(self) -> NonZeroU16 {
        self.frames
    }
}
