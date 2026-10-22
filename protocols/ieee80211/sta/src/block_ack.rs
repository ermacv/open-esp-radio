//! Receive Block Ack negotiation, inactivity and reorder for one association.
//!
//! The integration supplies an existing bank token and retained-frame slots.
//! This module allocates no frame backing and never reads a clock.

use oer_ieee80211_upper_mac::block_ack::{
    RxReorderBuffer, RxReorderError, RxReorderMpdu, RxReorderRelease,
};
use oer_ieee80211_wire::{
    block_ack::frame::{AddbaParameters, BlockAckAction},
    sequence::SequenceNumber,
};
use oer_time::{Duration, Instant};

/// IEEE 802.11 ADDBA response status, separate from a local parsing error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum AddbaStatus {
    Success = 0,
    RequestDeclined = 37,
    InvalidParameters = 38,
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
    pub fn encode(self) -> Option<[u8; 9]> {
        let parameters = AddbaParameters::from_bytes([0; 2])
            .with_immediate(true)
            .with_amsdu(self.amsdu)
            .with_tid(self.tid)
            .ok()?
            .with_window(self.window)
            .ok()?;
        let [s0, s1] = (self.status as u16).to_le_bytes();
        let [p0, p1] = parameters.to_bytes();
        let [t0, t1] = self.timeout_tu.to_le_bytes();
        Some([3, 1, self.dialog_token, s0, s1, p0, p1, t0, t1])
    }
}

/// Validated recipient parameters, before any hardware publication.
pub struct RxBlockAckNegotiation {
    response: AddbaResponse,
    starting_sequence: SequenceNumber,
    inactivity: Option<Instant>,
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
            let Some(deadline) = now.checked_add(inactivity_duration(timeout_tu)) else {
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
}

/// One affine hardware agreement beside its software sequence space.
pub struct RxBlockAckSession<A, const WINDOW: usize, const SLOTS: usize> {
    agreement: A,
    reorder: RxReorderBuffer<WINDOW, SLOTS>,
    inactivity: Option<Instant>,
    gap: Option<Instant>,
    timeout_tu: u16,
    tid: u8,
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
                timeout_tu: negotiation.response.timeout_tu,
                tid: negotiation.response.tid,
            }),
            Err(reason) => Err((agreement, reason)),
        }
    }

    pub const fn tid(&self) -> u8 {
        self.tid
    }

    pub fn deadline(&self) -> Option<Instant> {
        crate::time::earliest(self.inactivity, self.gap)
    }

    pub fn expired(&self, now: Instant) -> bool {
        self.inactivity.is_some_and(|deadline| now >= deadline)
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
        let gap = now
            .checked_add(gap_timeout)
            .ok_or(RxBlockAckError::DeadlineOverflow)?;
        let released = self
            .reorder
            .ingest(frame)
            .map_err(RxBlockAckError::Reorder)?;
        self.inactivity = inactivity;
        self.update_gap(gap);
        Ok(released)
    }

    /// The same hole keeps its original deadline under later RX traffic.
    fn update_gap(&mut self, next: Instant) {
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
        if self.gap.is_none_or(|deadline| now < deadline) {
            return Ok(None);
        }
        let next = now
            .checked_add(gap_timeout)
            .ok_or(RxBlockAckError::DeadlineOverflow)?;
        let released = self.reorder.expire_gap();
        self.gap = None;
        self.update_gap(next);
        Ok(Some(released))
    }

    /// Return every retained frame in order and the same bank token once.
    pub fn stop(mut self) -> (A, RxReorderRelease<WINDOW>) {
        (self.agreement, self.reorder.stop())
    }

    fn next_inactivity(&self, now: Instant) -> Result<Option<Instant>, RxBlockAckError> {
        if self.timeout_tu == 0 {
            Ok(None)
        } else {
            now.checked_add(inactivity_duration(self.timeout_tu))
                .map(Some)
                .ok_or(RxBlockAckError::DeadlineOverflow)
        }
    }
}

fn inactivity_duration(timeout_tu: u16) -> Duration {
    Duration::from_micros(u64::from(timeout_tu).saturating_mul(1024))
}

#[cfg(test)]
mod tests;
