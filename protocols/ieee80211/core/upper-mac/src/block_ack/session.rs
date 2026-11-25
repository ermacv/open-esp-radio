//! One bounded TX Block Ack negotiation and operational session.

use oer_portable_bounded::Index;
use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee80211_wire::block_ack::frame::{
    ADDBA_ACTION_BODY_LEN, AddbaRequestError, BlockAckAction, encode_addba_request,
    parse_block_ack_action,
};
use oer_protocols_ieee80211_wire::management::DialogToken;
use oer_protocols_ieee80211_wire::qos::WmmUserPriority;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;
use oer_protocols_ieee80211_wire::status::StatusCode;

/// Largest originator window represented by the delivery/expiry bitmaps.
pub const MAX_TX_BLOCK_ACK_WINDOW: u16 = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBlockAckError {
    InvalidWindow(u16),
    ZeroTimeout,
    DeadlineOverflow,
    MalformedResponse,
    UnexpectedResponse,
    DelayedPolicyUnsupported,
    WindowExceedsCapacity(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckConfig {
    pub tid: WmmUserPriority,
    pub window: u16,
    pub timeout_tu: u16,
    /// How long a negotiation waits for the peer's ADDBA response.
    pub negotiation_timeout: Duration,
    pub amsdu: bool,
}

impl TxBlockAckConfig {
    pub const fn validate(self) -> Result<Self, TxBlockAckError> {
        if self.window == 0 || self.window > MAX_TX_BLOCK_ACK_WINDOW {
            return Err(TxBlockAckError::InvalidWindow(self.window));
        }
        if self.negotiation_timeout.as_micros() == 0 {
            return Err(TxBlockAckError::ZeroTimeout);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckAlarm {
    pub generation: u32,
    /// When the outstanding negotiation times out.
    pub deadline: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddbaRequest {
    pub generation: u32,
    pub dialog_token: DialogToken,
    pub starting_sequence: SequenceNumber,
    pub body: [u8; ADDBA_ACTION_BODY_LEN],
    pub alarm: TxBlockAckAlarm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationalTxBlockAck {
    pub tid: WmmUserPriority,
    pub window: u16,
    pub timeout_tu: u16,
    /// Current originator-window head, advanced by delivery and expiry.
    pub starting_sequence: SequenceNumber,
    pub amsdu: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBlockAckResponse {
    Operational(OperationalTxBlockAck),
    Rejected(StatusCode),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxBlockAckPhase {
    Idle,
    Awaiting {
        dialog_token: DialogToken,
        starting_sequence: SequenceNumber,
    },
    Operational {
        agreement: OperationalTxBlockAck,
        delivered: u64,
        expired: u64,
    },
}

/// One statically owned TX BlockAck agreement for one user priority's TID.
///
/// Progress processes at most 64 sequence positions. Timer expiry is an
/// externally delivered edge; this type never reads time, sleeps, or retries.
pub struct TxBlockAckSession {
    config: TxBlockAckConfig,
    generation: u32,
    next_dialog_token: DialogToken,
    phase: TxBlockAckPhase,
}

impl TxBlockAckSession {
    pub const fn new(config: TxBlockAckConfig) -> Result<Self, TxBlockAckError> {
        let config = match config.validate() {
            Ok(config) => config,
            Err(error) => return Err(error),
        };
        Ok(Self {
            config,
            generation: 0,
            next_dialog_token: DialogToken::new(1),
            phase: TxBlockAckPhase::Idle,
        })
    }

    pub fn begin(
        &mut self,
        starting_sequence: SequenceNumber,
        now: Instant,
    ) -> Result<AddbaRequest, TxBlockAckError> {
        let dialog_token = self.next_dialog_token;
        self.next_dialog_token = dialog_token.next_nonzero();
        self.begin_with_dialog_token(starting_sequence, now, dialog_token)
    }

    /// Start negotiation with a token supplied by a shared multi-TID owner.
    ///
    /// A caller that has more than one live TID must obtain this value from
    /// its shared token-allocation policy. The session still exclusively owns
    /// its timeout generation, starting sequence and operational agreement.
    pub fn begin_with_dialog_token(
        &mut self,
        starting_sequence: SequenceNumber,
        now: Instant,
        dialog_token: DialogToken,
    ) -> Result<AddbaRequest, TxBlockAckError> {
        let deadline = now
            .checked_add(self.config.negotiation_timeout)
            .ok_or(TxBlockAckError::DeadlineOverflow)?;
        let body = encode_addba_request(
            dialog_token,
            self.config.tid.tid(),
            self.config.window,
            self.config.timeout_tu,
            if self.config.amsdu {
                oer_protocols_ieee80211_wire::block_ack::frame::AmsduSupport::Supported
            } else {
                oer_protocols_ieee80211_wire::block_ack::frame::AmsduSupport::NotSupported
            },
            starting_sequence,
        )
        .map_err(|error| match error {
            AddbaRequestError::InvalidWindow(window) => TxBlockAckError::InvalidWindow(window),
        })?;
        self.generation = next_generation(self.generation);
        self.phase = TxBlockAckPhase::Awaiting {
            dialog_token,
            starting_sequence,
        };

        let alarm = TxBlockAckAlarm {
            generation: self.generation,
            deadline,
        };
        Ok(AddbaRequest {
            generation: self.generation,
            dialog_token,
            starting_sequence,
            body,
            alarm,
        })
    }

    pub fn on_response(&mut self, body: &[u8]) -> Result<TxBlockAckResponse, TxBlockAckError> {
        // The nine-byte ADDBA response is a fixed prefix, not the complete
        // action-body length. An HE peer may append an ADDBA Extension IE
        // (element 159). Linux `net/mac80211/agg-rx.c`::
        // `ieee80211_send_addba_resp` does exactly that after the fixed
        // response fields. A controlled AX211 HE20 hardware test reached
        // `parse_block_ack_action(AddbaResponse)` and then failed only an
        // exact-length check. We deliberately consume only the fixed
        // prefix here: the negotiated low ten-bit window remains bounded by
        // `self.config.window`; a future owner of extended (>1024) windows
        // must parse the IE separately.
        let action = parse_block_ack_action(body).ok_or(TxBlockAckError::MalformedResponse)?;
        self.on_response_action(action)
    }

    /// Apply the fixed fields of an already parsed ADDBA response.
    pub fn on_response_action(
        &mut self,
        action: BlockAckAction,
    ) -> Result<TxBlockAckResponse, TxBlockAckError> {
        let BlockAckAction::AddbaResponse {
            dialog_token: response_dialog_token,
            status,
            parameters,
            timeout_tu,
        } = action
        else {
            return Err(TxBlockAckError::MalformedResponse);
        };
        let TxBlockAckPhase::Awaiting {
            dialog_token,
            starting_sequence,
        } = self.phase
        else {
            return Err(TxBlockAckError::UnexpectedResponse);
        };
        if response_dialog_token != dialog_token {
            return Err(TxBlockAckError::UnexpectedResponse);
        }
        let (tid, immediate, amsdu, window) = (
            parameters.tid(),
            parameters.policy()
                == oer_protocols_ieee80211_wire::block_ack::frame::BlockAckPolicy::Immediate,
            parameters.amsdu(),
            parameters.window(),
        );

        if status != StatusCode::SUCCESS {
            self.phase = TxBlockAckPhase::Idle;
            self.generation = next_generation(self.generation);
            return Ok(TxBlockAckResponse::Rejected(status));
        }

        if !immediate {
            return Err(TxBlockAckError::DelayedPolicyUnsupported);
        }
        if tid != self.config.tid.tid() {
            return Err(TxBlockAckError::UnexpectedResponse);
        }
        if window == 0 || window > self.config.window {
            return Err(TxBlockAckError::WindowExceedsCapacity(window));
        }
        let agreement = OperationalTxBlockAck {
            tid: self.config.tid,
            window,
            timeout_tu,
            starting_sequence,
            amsdu: self.config.amsdu && amsdu,
        };
        self.phase = TxBlockAckPhase::Operational {
            agreement,
            delivered: 0,
            expired: 0,
        };
        self.generation = next_generation(self.generation);
        Ok(TxBlockAckResponse::Operational(agreement))
    }

    /// Consume one exact async timer edge. Returns true only when it cancelled
    /// the currently outstanding negotiation.
    pub fn on_alarm(&mut self, alarm: TxBlockAckAlarm) -> bool {
        if alarm.generation != self.generation
            || !matches!(self.phase, TxBlockAckPhase::Awaiting { .. })
        {
            return false;
        }
        self.phase = TxBlockAckPhase::Idle;
        self.generation = next_generation(self.generation);
        true
    }

    /// Mark delivered MSDUs: bit i names `starting_sequence + i`.
    /// Return a BAR SSN if advancing over this delivery also passes an
    /// earlier expiry. Non-head marks remain until the head reaches them.
    pub fn on_delivered(
        &mut self,
        starting_sequence: SequenceNumber,
        bitmap: u64,
    ) -> Option<SequenceNumber> {
        self.progress(starting_sequence, bitmap, Marked::Delivered)
    }

    /// Mark expired MSDUs: bit i names `starting_sequence + i`.
    /// Apply the whole report before advancing, returning one final BAR
    /// SSN if the window passes any expired MSDU. Inactive agreements and
    /// positions outside the current window have no effect.
    pub fn on_expired(
        &mut self,
        starting_sequence: SequenceNumber,
        bitmap: u64,
    ) -> Option<SequenceNumber> {
        self.progress(starting_sequence, bitmap, Marked::Expired)
    }

    fn progress(
        &mut self,
        starting_sequence: SequenceNumber,
        bitmap: u64,
        marked: Marked,
    ) -> Option<SequenceNumber> {
        let is_expired = marked == Marked::Expired;
        let TxBlockAckPhase::Operational {
            agreement,
            delivered,
            expired,
        } = &mut self.phase
        else {
            return None;
        };
        for offset in 0..MAX_TX_BLOCK_ACK_WINDOW {
            if bitmap & (1_u64 << offset) == 0 {
                continue;
            }
            let sequence = starting_sequence.wrapping_add(offset);
            let distance = agreement.starting_sequence.forward_distance(sequence);
            if distance >= agreement.window {
                continue;
            }
            // Validation bounds the window by 64, so distance fits the mask.
            let bit = 1_u64 << distance;
            // Terminal marks are idempotent; a late report cannot revive one.
            if (*delivered | *expired) & bit != 0 {
                continue;
            }
            if is_expired {
                *expired |= bit;
            } else {
                *delivered |= bit;
            }
        }
        let run = Index::<65>::trailing_ones(*delivered | *expired);
        let advance = run.to_u32();
        let needs_bar = expired.trailing_zeros() < advance;
        if advance == u64::BITS {
            *delivered = 0;
            *expired = 0;
        } else {
            *delivered >>= advance;
            *expired >>= advance;
        }
        agreement.starting_sequence = agreement.starting_sequence.wrapping_add(run.to_u16());
        needs_bar.then_some(agreement.starting_sequence)
    }

    pub fn stop(&mut self) {
        self.phase = TxBlockAckPhase::Idle;
        self.generation = next_generation(self.generation);
    }

    /// Identity of the current phase. Every negotiation, response, alarm and
    /// stop advances it, so an unchanged generation of an operational session
    /// names exactly the agreement that was operational when it was read.
    pub const fn generation(&self) -> u32 {
        self.generation
    }

    pub const fn operational(&self) -> Option<OperationalTxBlockAck> {
        match self.phase {
            TxBlockAckPhase::Operational { agreement, .. } => Some(agreement),
            _ => None,
        }
    }

    pub const fn is_awaiting(&self) -> bool {
        matches!(self.phase, TxBlockAckPhase::Awaiting { .. })
    }

    /// Dialog Token currently owned by an outstanding negotiation.
    ///
    /// A multi-TID dispatcher can use this before calling [`Self::on_response`]
    /// so a response is delivered to exactly one session.
    pub const fn awaiting_dialog_token(&self) -> Option<DialogToken> {
        match self.phase {
            TxBlockAckPhase::Awaiting { dialog_token, .. } => Some(dialog_token),
            _ => None,
        }
    }
}

const fn next_generation(current: u32) -> u32 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

/// What a report marks of an agreement's MSDUs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Marked {
    Delivered,
    Expired,
}

#[cfg(test)]
mod tests;
