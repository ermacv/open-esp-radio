//! One bounded TX Block Ack negotiation and operational session.

use super::frame::{
    ADDBA_ACTION_BODY_LEN, ADDBA_REQUEST_ACTION, BLOCK_ACK_CATEGORY, BlockAckAction,
    parse_block_ack_action,
};
use crate::sequence::SequenceNumber;
use oer_time::{Duration, Instant};

const BLOCK_ACK_WINDOW_FIELD_MAX: u16 = 0x03ff;

const BA_PARAMETER_AMSDU: u16 = 1;
const BA_PARAMETER_IMMEDIATE: u16 = 1 << 1;
const BA_PARAMETER_TID_SHIFT: u32 = 2;
const BA_PARAMETER_WINDOW_SHIFT: u32 = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBlockAckError {
    InvalidTid(u8),
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
    pub tid: u8,
    pub window: u16,
    pub timeout_tu: u16,
    /// How long a negotiation waits for the peer's ADDBA response.
    pub negotiation_timeout: Duration,
    pub amsdu: bool,
}

impl TxBlockAckConfig {
    pub const fn validate(self) -> Result<Self, TxBlockAckError> {
        if self.tid > 15 {
            return Err(TxBlockAckError::InvalidTid(self.tid));
        }
        if self.window == 0 || self.window > BLOCK_ACK_WINDOW_FIELD_MAX {
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

/// One BlockAck action Dialog Token supplied by a protocol owner.
///
/// Keeping construction private prevents independent per-TID sessions from
/// accidentally reusing a token while their negotiations overlap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckDialogToken(u8);

impl TxBlockAckDialogToken {
    /// Wrap a token selected by a multi-session allocation policy.
    ///
    /// All bit patterns fit the action-frame field. Whether zero is used and
    /// when a sequence wraps are policy decisions outside this protocol leaf.
    pub const fn from_value(value: u8) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddbaRequest {
    pub generation: u32,
    pub dialog_token: u8,
    pub starting_sequence: SequenceNumber,
    pub body: [u8; ADDBA_ACTION_BODY_LEN],
    pub alarm: TxBlockAckAlarm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationalTxBlockAck {
    pub tid: u8,
    pub window: u16,
    pub timeout_tu: u16,
    pub starting_sequence: SequenceNumber,
    pub amsdu: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBlockAckResponse {
    Operational(OperationalTxBlockAck),
    Rejected(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxBlockAckPhase {
    Idle,
    Awaiting {
        dialog_token: u8,
        starting_sequence: SequenceNumber,
    },
    Operational(OperationalTxBlockAck),
}

/// One statically owned TX BlockAck agreement for one QoS TID.
///
/// Every method performs a fixed number of loads/stores. Timer expiry is an
/// externally delivered edge; this type never reads time, sleeps, or retries.
pub struct TxBlockAckSession {
    config: TxBlockAckConfig,
    generation: u32,
    next_dialog_token: u8,
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
            next_dialog_token: 1,
            phase: TxBlockAckPhase::Idle,
        })
    }

    pub fn begin(
        &mut self,
        starting_sequence: SequenceNumber,
        now: Instant,
    ) -> Result<AddbaRequest, TxBlockAckError> {
        let dialog_token = TxBlockAckDialogToken(self.next_dialog_token);
        self.next_dialog_token = super::next_nonzero_dialog_token(dialog_token.value());
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
        dialog_token: TxBlockAckDialogToken,
    ) -> Result<AddbaRequest, TxBlockAckError> {
        let deadline = now
            .checked_add(self.config.negotiation_timeout)
            .ok_or(TxBlockAckError::DeadlineOverflow)?;
        self.generation = next_generation(self.generation);
        let dialog_token = dialog_token.value();
        self.phase = TxBlockAckPhase::Awaiting {
            dialog_token,
            starting_sequence,
        };

        let parameters =
            encode_ba_parameters(self.config.tid, self.config.window, self.config.amsdu);
        let sequence_control = starting_sequence.sequence_control();
        let mut body = [0_u8; ADDBA_ACTION_BODY_LEN];
        body[0] = BLOCK_ACK_CATEGORY;
        body[1] = ADDBA_REQUEST_ACTION;
        body[2] = dialog_token;
        body[3..5].copy_from_slice(&parameters.to_le_bytes());
        body[5..7].copy_from_slice(&self.config.timeout_tu.to_le_bytes());
        body[7..9].copy_from_slice(&sequence_control.to_le_bytes());

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

    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
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
    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    pub fn on_response_action(
        &mut self,
        action: BlockAckAction,
    ) -> Result<TxBlockAckResponse, TxBlockAckError> {
        let BlockAckAction::AddbaResponse {
            dialog_token: response_dialog_token,
            status,
            tid,
            immediate,
            amsdu,
            window,
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

        if status != 0 {
            self.phase = TxBlockAckPhase::Idle;
            self.generation = next_generation(self.generation);
            return Ok(TxBlockAckResponse::Rejected(status));
        }

        if !immediate {
            return Err(TxBlockAckError::DelayedPolicyUnsupported);
        }
        if tid != self.config.tid {
            return Err(TxBlockAckError::UnexpectedResponse);
        }
        if window == 0 || window > self.config.window {
            return Err(TxBlockAckError::WindowExceedsCapacity(window));
        }
        let agreement = OperationalTxBlockAck {
            tid,
            window,
            timeout_tu,
            starting_sequence,
            amsdu: self.config.amsdu && amsdu,
        };
        self.phase = TxBlockAckPhase::Operational(agreement);
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
            TxBlockAckPhase::Operational(agreement) => Some(agreement),
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
    pub const fn awaiting_dialog_token(&self) -> Option<u8> {
        match self.phase {
            TxBlockAckPhase::Awaiting { dialog_token, .. } => Some(dialog_token),
            _ => None,
        }
    }
}

const fn encode_ba_parameters(tid: u8, window: u16, amsdu: bool) -> u16 {
    ((amsdu as u16) * BA_PARAMETER_AMSDU)
        | BA_PARAMETER_IMMEDIATE
        | ((tid as u16) << BA_PARAMETER_TID_SHIFT)
        | (window << BA_PARAMETER_WINDOW_SHIFT)
}

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
const fn next_generation(current: u32) -> u32 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

#[cfg(test)]
mod tests;
