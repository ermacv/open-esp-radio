//! Receive Block Ack negotiation, inactivity and reorder for one association.
//!
//! The integration supplies an existing bank token and retained-frame slots.
//! This module allocates no frame backing and never reads a clock.

use super::{RxReorderBuffer, RxReorderError, RxReorderMpdu, RxReorderRelease};
use core::num::NonZeroU64;
use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee80211_wire::management::DialogToken;
use oer_protocols_ieee80211_wire::qos::Tid;
use oer_protocols_ieee80211_wire::{
    block_ack::frame::{
        ADDBA_ACTION_BODY_LEN, BlockAckAction, BlockAckPolicy, encode_addba_response,
    },
    sequence::SequenceNumber,
    status::StatusCode,
    tsf::TimeUnits,
};

/// IEEE 802.11 ADDBA response status, separate from a local parsing error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddbaStatus {
    Success,
    RequestDeclined,
    InvalidParameters,
}

impl AddbaStatus {
    /// The Status Code that says it.
    pub const fn code(self) -> StatusCode {
        match self {
            Self::Success => StatusCode::SUCCESS,
            Self::RequestDeclined => StatusCode::REQUEST_DECLINED,
            Self::InvalidParameters => StatusCode::INVALID_PARAMETERS,
        }
    }
}

/// The A-MSDU Supported subfield of a Block Ack Parameter Set (IEEE Std
/// 802.11-2020, 9.4.1.14): whether A-MSDUs may go under the agreement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Amsdu {
    Permitted,
    NotPermitted,
}

/// Whether the backend delivers an MPDU carrying an A-MSDU inside an
/// A-MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmsduInAmpdu {
    Delivered,
    NotDelivered,
}

/// Whether a hardware bank is free for a new agreement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BankRoom {
    Free,
    Exhausted,
}

/// Fixed fields of one parsed ADDBA Request, without another action variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxAddbaRequest {
    dialog_token: DialogToken,
    tid: Tid,
    policy: BlockAckPolicy,
    amsdu: Amsdu,
    window: u16,
    timeout: TimeUnits,
    starting_sequence: SequenceNumber,
}

impl RxAddbaRequest {
    pub const fn from_action(action: BlockAckAction) -> Option<Self> {
        match action {
            BlockAckAction::AddbaRequest {
                dialog_token,
                parameters,
                timeout,
                starting_sequence,
            } => Some(Self {
                dialog_token,
                tid: parameters.tid(),
                policy: parameters.policy(),
                amsdu: if parameters.amsdu() {
                    Amsdu::Permitted
                } else {
                    Amsdu::NotPermitted
                },
                window: parameters.window(),
                timeout,
                starting_sequence,
            }),
            _ => None,
        }
    }

    /// The TID the request names.
    pub const fn tid(&self) -> Tid {
        self.tid
    }

    /// The Buffer Size the request asks for, as received: zero asks the
    /// recipient to choose.
    pub const fn window(&self) -> u16 {
        self.window
    }
}

/// A response to one parsed request; refusal retains its dialog and TID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddbaResponse {
    pub dialog_token: DialogToken,
    pub status: AddbaStatus,
    pub tid: Tid,
    pub window: u16,
    pub timeout: TimeUnits,
    pub amsdu: bool,
}

impl AddbaResponse {
    pub fn encode(self) -> Option<[u8; ADDBA_ACTION_BODY_LEN]> {
        encode_addba_response(
            self.dialog_token,
            self.status.code(),
            self.tid,
            self.window,
            self.timeout,
            if self.amsdu {
                oer_protocols_ieee80211_wire::block_ack::frame::AmsduSupport::Supported
            } else {
                oer_protocols_ieee80211_wire::block_ack::frame::AmsduSupport::NotSupported
            },
        )
        .ok()
    }
}

/// Validated recipient parameters, before any hardware publication.
pub struct RxBlockAckNegotiation {
    response: AddbaResponse,
    starting_sequence: SequenceNumber,
    requested_window: u16,
    requested_amsdu: Amsdu,
    inactivity: Inactivity,
}

/// The agreement's inactivity deadline (Block Ack Timeout): off when the
/// agreement names no timeout, or at a nonzero microsecond instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Inactivity {
    Off,
    At(NonZeroU64),
}

/// The reorder gap's wait: idle while no hole is open, or until a nonzero
/// microsecond instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GapWait {
    Idle,
    Until(NonZeroU64),
}

impl RxBlockAckNegotiation {
    /// `capacity` is the integration's retained-frame budget, with a credit
    /// reserved for the MPDU that closes a gap. No bank means an explicit
    /// refusal, even when every peer parameter is otherwise supported.
    /// `amsdu_in_ampdu` is whether the backend delivers an MPDU carrying an
    /// A-MSDU inside an A-MPDU; the response grants A-MSDUs in the
    /// agreement only when the request asks and the backend delivers them.
    pub fn request(
        request: RxAddbaRequest,
        maximum_tid: Tid,
        amsdu_in_ampdu: AmsduInAmpdu,
        capacity: u16,
        bank: BankRoom,
        now: Instant,
    ) -> Result<Self, AddbaResponse> {
        let RxAddbaRequest {
            dialog_token,
            tid,
            policy,
            amsdu,
            window,
            timeout,
            starting_sequence,
        } = request;
        let mut response = AddbaResponse {
            dialog_token,
            status: AddbaStatus::RequestDeclined,
            tid,
            window,
            timeout,
            amsdu: false,
        };
        if window > 1023 {
            response.status = AddbaStatus::InvalidParameters;
            return Err(response);
        }
        if policy == BlockAckPolicy::Delayed
            || tid > maximum_tid
            || capacity == 0
            || bank == BankRoom::Exhausted
        {
            return Err(response);
        }
        let inactivity = if timeout == TimeUnits::ZERO {
            Inactivity::Off
        } else {
            let Some(deadline) = now
                .checked_add(inactivity_duration(timeout))
                .and_then(|at| NonZeroU64::new(at.as_micros()))
            else {
                return Err(response);
            };
            Inactivity::At(deadline)
        };
        response.status = AddbaStatus::Success;
        // Buffer Size zero asks for the HT default of 64 positions.
        // IEEE 802.11 ADDBA Parameter Set; Linux mac80211 agg-rx.c
        // __ieee80211_start_rx_ba_session applies the same default before
        // bounding it by the receiver's actual storage capacity.
        response.window = if window == 0 { 64 } else { window }.min(capacity).min(64);
        response.amsdu = amsdu == Amsdu::Permitted && amsdu_in_ampdu == AmsduInAmpdu::Delivered;
        Ok(Self {
            response,
            starting_sequence,
            requested_window: window,
            requested_amsdu: amsdu,
            inactivity,
        })
    }

    pub const fn response(&self) -> AddbaResponse {
        self.response
    }

    pub const fn starting_sequence(&self) -> SequenceNumber {
        self.starting_sequence
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxBlockAckError {
    Reorder(RxReorderError),
    DeadlineOverflow,
    ZeroGapTimeout,
}

/// One affine hardware agreement beside its software sequence space.
pub struct RxBlockAckSession<A, const WINDOW: usize, const SLOTS: usize> {
    agreement: A,
    reorder: RxReorderBuffer<WINDOW, SLOTS>,
    inactivity: Inactivity,
    gap: GapWait,
    starting_sequence: SequenceNumber,
    requested_window: u16,
    requested_amsdu: Amsdu,
    timeout: TimeUnits,
    tid: Tid,
    dialog_token: DialogToken,
    amsdu: bool,
}

impl<A, const WINDOW: usize, const SLOTS: usize> RxBlockAckSession<A, WINDOW, SLOTS> {
    /// An invalid geometry returns the unchanged bank owner.
    pub fn activate(
        negotiation: RxBlockAckNegotiation,
        agreement: A,
    ) -> Result<Self, (A, RxReorderError)> {
        match RxReorderBuffer::new(negotiation.starting_sequence, negotiation.response.window) {
            Ok(reorder) => Ok(Self {
                agreement,
                reorder,
                inactivity: negotiation.inactivity,
                gap: GapWait::Idle,
                starting_sequence: negotiation.starting_sequence,
                requested_window: negotiation.requested_window,
                requested_amsdu: negotiation.requested_amsdu,
                timeout: negotiation.response.timeout,
                tid: negotiation.response.tid,
                dialog_token: negotiation.response.dialog_token,
                amsdu: negotiation.response.amsdu,
            }),
            Err(reason) => Err((agreement, reason)),
        }
    }

    pub const fn tid(&self) -> Tid {
        self.tid
    }

    pub fn window(&self) -> u16 {
        self.reorder.window()
    }

    pub const fn dialog_token(&self) -> DialogToken {
        self.dialog_token
    }

    /// A retried dialog may repeat its response without resetting reorder.
    /// Changed parameters under that same dialog are explicitly refused.
    pub fn repeated_request(&self, request: RxAddbaRequest) -> Option<AddbaResponse> {
        if request.tid != self.tid || request.dialog_token != self.dialog_token {
            return None;
        }
        let identical = request.policy == BlockAckPolicy::Immediate
            && request.amsdu == self.requested_amsdu
            && request.window == self.requested_window
            && request.timeout == self.timeout
            && request.starting_sequence == self.starting_sequence;
        Some(if identical {
            self.response()
        } else {
            AddbaResponse {
                dialog_token: request.dialog_token,
                status: AddbaStatus::RequestDeclined,
                tid: request.tid,
                window: request.window,
                timeout: request.timeout,
                amsdu: false,
            }
        })
    }

    pub fn response(&self) -> AddbaResponse {
        AddbaResponse {
            dialog_token: self.dialog_token,
            status: AddbaStatus::Success,
            tid: self.tid,
            window: self.reorder.window(),
            timeout: self.timeout,
            amsdu: self.amsdu,
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        // Both deadlines are nonzero microsecond instants; the sooner wins.
        match (self.inactivity, self.gap) {
            (Inactivity::At(inactivity), GapWait::Until(gap)) => {
                Some(Instant::from_micros(inactivity.min(gap).get()))
            }
            (Inactivity::At(deadline), GapWait::Idle)
            | (Inactivity::Off, GapWait::Until(deadline)) => {
                Some(Instant::from_micros(deadline.get()))
            }
            (Inactivity::Off, GapWait::Idle) => None,
        }
    }

    pub fn expired(&self, now: Instant) -> bool {
        match self.inactivity {
            Inactivity::Off => false,
            Inactivity::At(deadline) => now.as_micros() >= deadline.get(),
        }
    }

    /// Whether an MPDU of `sequence` would be retained, not released at
    /// once: it does not close the window's hole.
    ///
    /// # Errors
    ///
    /// The sequence is a buffered duplicate.
    pub fn retains(&self, sequence: SequenceNumber) -> Result<bool, RxReorderError> {
        self.reorder.retains_on_ingest(sequence)
    }

    pub fn peer_delba(&self, action: BlockAckAction) -> bool {
        matches!(
            action,
            BlockAckAction::Delba { tid, initiator: oer_protocols_ieee80211_wire::block_ack::frame::DelbaInitiator::Originator, .. } if tid == self.tid
        )
    }

    /// Take the common in-order MPDU without a release list: `true` when
    /// it is the window's start and no buffered MPDU follows it, so it goes
    /// out at once and the window moves past it. Otherwise nothing changed
    /// and the MPDU needs [`Self::ingest`]. The agreement's inactivity
    /// restarts as for an ingested MPDU; no hole opens or closes.
    pub fn try_ingest_immediate(
        &mut self,
        now: Instant,
        frame: RxReorderMpdu,
    ) -> Result<bool, RxBlockAckError> {
        let inactivity = self.next_inactivity(now)?;
        let taken = self
            .reorder
            .try_ingest_immediate(frame)
            .map_err(RxBlockAckError::Reorder)?
            .is_some();
        if taken {
            self.inactivity = inactivity;
        }
        Ok(taken)
    }

    pub fn ingest(
        &mut self,
        now: Instant,
        frame: RxReorderMpdu,
        gap_timeout: Duration,
    ) -> Result<RxReorderRelease<WINDOW>, RxBlockAckError> {
        let inactivity = self.next_inactivity(now)?;
        let gap = gap_deadline(now, gap_timeout)?;
        let released = self
            .reorder
            .ingest(frame)
            .map_err(RxBlockAckError::Reorder)?;
        self.inactivity = inactivity;
        self.update_gap(gap);
        Ok(released)
    }

    /// The same hole keeps its original deadline under later RX traffic.
    fn update_gap(&mut self, next: NonZeroU64) {
        if self.reorder.occupied() == 0 {
            self.gap = GapWait::Idle;
        } else if self.gap == GapWait::Idle {
            self.gap = GapWait::Until(next);
        }
    }

    pub fn expire_gap(
        &mut self,
        now: Instant,
        gap_timeout: Duration,
    ) -> Result<Option<RxReorderRelease<WINDOW>>, RxBlockAckError> {
        match self.gap {
            GapWait::Idle => return Ok(None),
            GapWait::Until(deadline) if now.as_micros() < deadline.get() => return Ok(None),
            GapWait::Until(_) => {}
        }
        let next = gap_deadline(now, gap_timeout)?;
        let released = self.reorder.expire_gap();
        self.gap = GapWait::Idle;
        self.update_gap(next);
        Ok(Some(released))
    }

    /// A forward BAR releases retained MPDUs through the requested start,
    /// then the contiguous run. Stale BARs leave the gap timer unchanged.
    pub fn block_ack_request(
        &mut self,
        now: Instant,
        starting_sequence: SequenceNumber,
        gap_timeout: Duration,
    ) -> Result<Option<RxReorderRelease<WINDOW>>, RxBlockAckError> {
        let inactivity = self.next_inactivity(now)?;
        let next = gap_deadline(now, gap_timeout)?;
        let released = self.reorder.move_window_to(starting_sequence);
        self.inactivity = inactivity;
        if released.is_some() {
            self.gap = GapWait::Idle;
            self.update_gap(next);
        }
        Ok(released)
    }

    /// Return every retained frame in order and the same bank token once.
    pub fn stop(mut self) -> (A, RxReorderRelease<WINDOW>) {
        (self.agreement, self.reorder.stop())
    }

    fn next_inactivity(&self, now: Instant) -> Result<Inactivity, RxBlockAckError> {
        if self.timeout == TimeUnits::ZERO {
            Ok(Inactivity::Off)
        } else {
            now.checked_add(inactivity_duration(self.timeout))
                .and_then(|at| NonZeroU64::new(at.as_micros()))
                .map(Inactivity::At)
                .ok_or(RxBlockAckError::DeadlineOverflow)
        }
    }
}

// Both relative intervals are nonzero, so a successful absolute deadline
// cannot be epoch zero. The nonzero representation preserves that proof and
// avoids a second machine word for each optional timer on RV32.
fn gap_deadline(now: Instant, timeout: Duration) -> Result<NonZeroU64, RxBlockAckError> {
    if timeout.as_micros() == 0 {
        return Err(RxBlockAckError::ZeroGapTimeout);
    }
    now.checked_add(timeout)
        .and_then(|at| NonZeroU64::new(at.as_micros()))
        .ok_or(RxBlockAckError::DeadlineOverflow)
}

/// The inactivity timeout on the station's own clock: the recipient counts
/// it on the image's monotonic time (IEEE Std 802.11-2020 10.25.6.4).
fn inactivity_duration(timeout: TimeUnits) -> Duration {
    timeout.span::<oer_portable_time::Monotonic>()
}

#[cfg(test)]
mod tests;
