//! Receive Block Ack negotiation, inactivity and reorder for one association.
//!
//! The integration supplies an existing bank token and retained-frame slots.
//! This module allocates no frame backing and never reads a clock.

use core::num::NonZeroU64;
use oer_ieee80211_upper_mac::block_ack::{
    RxReorderBuffer, RxReorderError, RxReorderMpdu, RxReorderRelease,
};
use oer_ieee80211_wire::{
    block_ack::frame::{ADDBA_ACTION_BODY_LEN, BlockAckAction, encode_addba_response},
    sequence::SequenceNumber,
    status::status_code,
    tsf::time_units_micros,
};
use oer_time::{Duration, Instant};

/// IEEE 802.11 ADDBA response status, separate from a local parsing error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum AddbaStatus {
    Success = status_code::SUCCESS,
    RequestDeclined = status_code::REQUEST_DECLINED,
    InvalidParameters = status_code::INVALID_PARAMETERS,
}

/// Fixed fields of one parsed ADDBA Request, without another action variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxAddbaRequest {
    dialog_token: u8,
    tid: u8,
    immediate: bool,
    amsdu: bool,
    window: u16,
    timeout_tu: u16,
    starting_sequence: SequenceNumber,
}

impl RxAddbaRequest {
    pub const fn from_action(action: BlockAckAction) -> Option<Self> {
        match action {
            BlockAckAction::AddbaRequest {
                dialog_token,
                tid,
                immediate,
                amsdu,
                window,
                timeout_tu,
                starting_sequence,
            } => Some(Self {
                dialog_token,
                tid,
                immediate,
                amsdu,
                window,
                timeout_tu,
                starting_sequence,
            }),
            _ => None,
        }
    }
}

/// A response to one parsed request; refusal retains its dialog and TID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddbaResponse {
    pub dialog_token: u8,
    pub status: AddbaStatus,
    pub tid: u8,
    pub window: u16,
    pub timeout_tu: u16,
    pub amsdu: bool,
}

impl AddbaResponse {
    pub fn encode(self) -> Option<[u8; ADDBA_ACTION_BODY_LEN]> {
        encode_addba_response(
            self.dialog_token,
            self.status as u16,
            self.tid,
            self.window,
            self.timeout_tu,
            self.amsdu,
        )
        .ok()
    }
}

/// Validated recipient parameters, before any hardware publication.
pub struct RxBlockAckNegotiation {
    response: AddbaResponse,
    starting_sequence: SequenceNumber,
    requested_window: u16,
    inactivity: Option<NonZeroU64>,
}

impl RxBlockAckNegotiation {
    /// `capacity` is the integration's retained-frame budget, with a credit
    /// reserved for the MPDU that closes a gap. No bank means an explicit
    /// refusal, even when every peer parameter is otherwise supported.
    pub fn request(
        request: RxAddbaRequest,
        maximum_tid: u8,
        capacity: u16,
        bank_available: bool,
        now: Instant,
    ) -> Result<Self, AddbaResponse> {
        let RxAddbaRequest {
            dialog_token,
            tid,
            immediate,
            amsdu,
            window,
            timeout_tu,
            starting_sequence,
        } = request;
        let mut response = AddbaResponse {
            dialog_token,
            status: AddbaStatus::RequestDeclined,
            tid,
            window,
            timeout_tu,
            amsdu: false,
        };
        if window > 1023 {
            response.status = AddbaStatus::InvalidParameters;
            return Err(response);
        }
        if !immediate || tid > maximum_tid || capacity == 0 || !bank_available {
            return Err(response);
        }
        let inactivity = if timeout_tu == 0 {
            None
        } else {
            let Some(deadline) = now
                .checked_add(inactivity_duration(timeout_tu))
                .and_then(|at| NonZeroU64::new(at.as_micros()))
            else {
                return Err(response);
            };
            Some(deadline)
        };
        response.status = AddbaStatus::Success;
        // Buffer Size zero asks for the HT default of 64 positions.
        // IEEE 802.11 ADDBA Parameter Set; Linux mac80211 agg-rx.c
        // __ieee80211_start_rx_ba_session applies the same default before
        // bounding it by the receiver's actual storage capacity.
        response.window = if window == 0 { 64 } else { window }.min(capacity).min(64);
        response.amsdu = amsdu;
        Ok(Self {
            response,
            starting_sequence,
            requested_window: window,
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
    inactivity: Option<NonZeroU64>,
    gap: Option<NonZeroU64>,
    starting_sequence: SequenceNumber,
    requested_window: u16,
    timeout_tu: u16,
    tid: u8,
    dialog_token: u8,
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
                gap: None,
                starting_sequence: negotiation.starting_sequence,
                requested_window: negotiation.requested_window,
                timeout_tu: negotiation.response.timeout_tu,
                tid: negotiation.response.tid,
                dialog_token: negotiation.response.dialog_token,
                amsdu: negotiation.response.amsdu,
            }),
            Err(reason) => Err((agreement, reason)),
        }
    }

    pub const fn tid(&self) -> u8 {
        self.tid
    }

    pub fn window(&self) -> u16 {
        self.reorder.window()
    }

    pub const fn dialog_token(&self) -> u8 {
        self.dialog_token
    }

    /// A retried dialog may repeat its response without resetting reorder.
    /// Changed parameters under that same dialog are explicitly refused.
    pub fn repeated_request(&self, request: RxAddbaRequest) -> Option<AddbaResponse> {
        if request.tid != self.tid || request.dialog_token != self.dialog_token {
            return None;
        }
        let identical = request.immediate
            && request.amsdu == self.amsdu
            && request.window == self.requested_window
            && request.timeout_tu == self.timeout_tu
            && request.starting_sequence == self.starting_sequence;
        Some(if identical {
            self.response()
        } else {
            AddbaResponse {
                dialog_token: request.dialog_token,
                status: AddbaStatus::RequestDeclined,
                tid: request.tid,
                window: request.window,
                timeout_tu: request.timeout_tu,
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
            timeout_tu: self.timeout_tu,
            amsdu: self.amsdu,
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        crate::time::earliest(
            self.inactivity.map(|at| Instant::from_micros(at.get())),
            self.gap.map(|at| Instant::from_micros(at.get())),
        )
    }

    pub fn expired(&self, now: Instant) -> bool {
        self.inactivity
            .is_some_and(|deadline| now.as_micros() >= deadline.get())
    }

    pub fn peer_delba(&self, action: BlockAckAction) -> bool {
        matches!(action, BlockAckAction::Delba { tid, initiator: true, .. } if tid == self.tid)
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
            self.gap = None;
        } else {
            self.gap.get_or_insert(next);
        }
    }

    pub fn expire_gap(
        &mut self,
        now: Instant,
        gap_timeout: Duration,
    ) -> Result<Option<RxReorderRelease<WINDOW>>, RxBlockAckError> {
        if self
            .gap
            .is_none_or(|deadline| now.as_micros() < deadline.get())
        {
            return Ok(None);
        }
        let next = gap_deadline(now, gap_timeout)?;
        let released = self.reorder.expire_gap();
        self.gap = None;
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
            self.gap = None;
            self.update_gap(next);
        }
        Ok(released)
    }

    /// Return every retained frame in order and the same bank token once.
    pub fn stop(mut self) -> (A, RxReorderRelease<WINDOW>) {
        (self.agreement, self.reorder.stop())
    }

    fn next_inactivity(&self, now: Instant) -> Result<Option<NonZeroU64>, RxBlockAckError> {
        if self.timeout_tu == 0 {
            Ok(None)
        } else {
            now.checked_add(inactivity_duration(self.timeout_tu))
                .and_then(|at| NonZeroU64::new(at.as_micros()))
                .map(Some)
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

fn inactivity_duration(timeout_tu: u16) -> Duration {
    Duration::from_micros(time_units_micros(timeout_tu))
}

#[cfg(test)]
mod tests;
