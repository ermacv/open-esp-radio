//! The originator of one peer's TX Block Ack agreements: one agreement per
//! TID its policy names, at most `TIDS` (the station's eight, an access
//! point's one per peer), a Dialog Token sequence the agreements share, and a
//! bounded number of negotiation attempts per TID, spaced by a retry
//! interval.
//!
//! Each agreement is a [`TxBlockAckSession`]. Which TIDs an originator
//! negotiates and how its Dialog Tokens advance is the integrator's policy
//! ([`TxBlockAckOriginatorPolicy`]); how many attempts a negotiation gets
//! and how long a failed one waits is the caller's [`TxBlockAckRetry`],
//! given when it queues the negotiation. The originator reads no time and
//! sends nothing: the caller hands it the time, asks it which negotiation
//! is due when it has traffic for the peer ([`TxBlockAckOriginator::take_pending`]),
//! sends the request bodies it returns and reports their outcome. A request
//! that did not leave and a response that did not come each use one attempt
//! and queue the next after the interval; the peer's answer, an agreement or
//! a refusal, ends them.

use oer_time::{Duration, Instant};

use super::{
    AddbaRequest, BlockAckAction, OperationalTxBlockAck, TxBlockAckAlarm, TxBlockAckConfig,
    TxBlockAckDialogToken, TxBlockAckError, TxBlockAckResponse, TxBlockAckSession,
    parse_block_ack_action,
};
use crate::sequence::SequenceNumber;

/// The QoS TIDs: the most agreements one originator owns.
pub const TX_BLOCK_ACK_MAX_TIDS: usize = 8;

/// The Dialog Token after `current` in a sequence that skips zero.
pub const fn next_nonzero_dialog_token(current: u8) -> u8 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

/// Which agreements an originator owns and how its Dialog Tokens advance.
#[derive(Clone, Copy, Debug)]
pub struct TxBlockAckOriginatorPolicy {
    /// The TIDs in the order their negotiations go out; at most the
    /// originator's `TIDS`, each below [`TX_BLOCK_ACK_MAX_TIDS`].
    pub tids: &'static [u8],
    /// The Dialog Token of the first request.
    pub first_dialog_token: u8,
    /// The Dialog Token after one.
    pub next_dialog_token: fn(u8) -> u8,
}

/// The agreement parameters every TID of an originator requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckOriginatorConfig {
    pub window: u16,
    /// How long a negotiation waits for the ADDBA Response.
    pub negotiation_timeout: Duration,
    /// The TIDs whose agreements may carry A-MSDUs, one bit per TID.
    pub amsdu_tids: u8,
}

/// How many attempts a queued negotiation gets and how long a failed one
/// waits before the next is due.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckRetry {
    pub attempts: u8,
    pub interval: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBlockAckOriginatorError {
    /// The policy names more TIDs than the originator owns.
    TooManyTids,
    /// The policy names this TID twice, or the TID is outside the originator.
    UnsupportedTid(u8),
    MalformedResponse,
    Session {
        tid: u8,
        error: TxBlockAckError,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckOriginatorResponse {
    pub tid: u8,
    pub response: TxBlockAckResponse,
}

/// A received ADDBA Response against the negotiations the originator owns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBlockAckResponseDisposition {
    Matched(TxBlockAckOriginatorResponse),
    /// The response names no live negotiation, such as one that already
    /// timed out: it changes no agreement and does not end the link.
    StaleDialogToken(u8),
}

struct Agreement {
    tid: u8,
    session: TxBlockAckSession,
    alarm: Option<TxBlockAckAlarm>,
    /// When the next negotiation is due; [`Instant::EPOCH`] at once.
    due: Option<Instant>,
    /// Negotiations left; a peer's answer ends them.
    attempts: u8,
}

/// One peer's TX Block Ack agreements, at most `TIDS`.
pub struct TxBlockAckOriginator<const TIDS: usize> {
    agreements: [Option<Agreement>; TIDS],
    /// How long a failed negotiation waits before the next is due.
    retry_interval: Duration,
    next_dialog_token: u8,
    advance_dialog_token: fn(u8) -> u8,
}

impl<const TIDS: usize> TxBlockAckOriginator<TIDS> {
    /// The idle agreements of `policy`'s TIDs, each requesting `config`.
    pub fn new(
        policy: TxBlockAckOriginatorPolicy,
        config: TxBlockAckOriginatorConfig,
    ) -> Result<Self, TxBlockAckOriginatorError> {
        if policy.tids.len() > TIDS {
            return Err(TxBlockAckOriginatorError::TooManyTids);
        }
        let mut agreements = [const { None }; TIDS];
        for (index, &tid) in policy.tids.iter().enumerate() {
            if usize::from(tid) >= TX_BLOCK_ACK_MAX_TIDS || policy.tids[..index].contains(&tid) {
                return Err(TxBlockAckOriginatorError::UnsupportedTid(tid));
            }
            let session = TxBlockAckSession::new(TxBlockAckConfig {
                tid,
                window: config.window,
                timeout_tu: 0,
                negotiation_timeout: config.negotiation_timeout,
                amsdu: config.amsdu_tids & (1 << tid) != 0,
            })
            .map_err(|error| TxBlockAckOriginatorError::Session { tid, error })?;
            agreements[index] = Some(Agreement {
                tid,
                session,
                alarm: None,
                due: None,
                attempts: 0,
            });
        }
        Ok(Self {
            agreements,
            retry_interval: Duration::ZERO,
            next_dialog_token: policy.first_dialog_token,
            advance_dialog_token: policy.next_dialog_token,
        })
    }

    fn agreement(&self, tid: u8) -> Option<&Agreement> {
        self.agreements
            .iter()
            .flatten()
            .find(|agreement| agreement.tid == tid)
    }

    fn agreement_mut(&mut self, tid: u8) -> Option<&mut Agreement> {
        self.agreements
            .iter_mut()
            .flatten()
            .find(|agreement| agreement.tid == tid)
    }

    /// The TIDs in the policy's order.
    pub fn tids(&self) -> impl Iterator<Item = u8> + '_ {
        self.agreements
            .iter()
            .flatten()
            .map(|agreement| agreement.tid)
    }

    /// Queue a negotiation of every TID, due at once, with `retry`'s
    /// attempts and interval; none when it allows no attempt.
    pub fn queue_initial(&mut self, retry: TxBlockAckRetry) {
        self.retry_interval = retry.interval;
        for agreement in self.agreements.iter_mut().flatten() {
            agreement.due = (retry.attempts != 0).then_some(Instant::EPOCH);
            agreement.attempts = retry.attempts;
        }
    }

    /// Whether a negotiation waits to be sent, due or not.
    pub fn has_pending(&self) -> bool {
        self.agreements
            .iter()
            .flatten()
            .any(|agreement| agreement.due.is_some())
    }

    /// When the earliest waiting negotiation is due.
    pub fn next_pending_deadline(&self) -> Option<Instant> {
        self.agreements
            .iter()
            .flatten()
            .filter_map(|agreement| agreement.due)
            .min()
    }

    /// The next TID whose negotiation is due at `now`, in the policy's
    /// order; taking it uses one of its attempts. The caller then
    /// [`Self::begin`]s it.
    pub fn take_pending(&mut self, now: Instant) -> Option<u8> {
        let agreement = self
            .agreements
            .iter_mut()
            .flatten()
            .find(|agreement| agreement.due.is_some_and(|due| due <= now))?;
        agreement.due = None;
        agreement.attempts = agreement.attempts.saturating_sub(1);
        Some(agreement.tid)
    }

    /// Begin the negotiation of `tid` from `starting_sequence`; the request
    /// owns its encoded action body.
    pub fn begin(
        &mut self,
        tid: u8,
        starting_sequence: SequenceNumber,
        now: Instant,
    ) -> Result<AddbaRequest, TxBlockAckOriginatorError> {
        let token = self.next_dialog_token;
        let agreement = self
            .agreement_mut(tid)
            .ok_or(TxBlockAckOriginatorError::UnsupportedTid(tid))?;
        let request = agreement
            .session
            .begin_with_dialog_token(
                starting_sequence,
                now,
                TxBlockAckDialogToken::from_value(token),
            )
            .map_err(|error| TxBlockAckOriginatorError::Session { tid, error })?;
        agreement.alarm = Some(request.alarm);
        self.next_dialog_token = (self.advance_dialog_token)(token);
        Ok(request)
    }

    /// The request of `tid` did not leave at `now`: its negotiation ends,
    /// and the next one is due after the interval while attempts remain.
    pub fn transmit_failed(&mut self, tid: u8, now: Instant) {
        let interval = self.retry_interval;
        if let Some(agreement) = self.agreement_mut(tid) {
            agreement.session.stop();
            agreement.alarm = None;
            agreement.retry(now, interval);
        }
    }

    /// Route one ADDBA Response body by its Dialog Token.
    pub fn on_response(
        &mut self,
        body: &[u8],
    ) -> Result<TxBlockAckResponseDisposition, TxBlockAckOriginatorError> {
        let action =
            parse_block_ack_action(body).ok_or(TxBlockAckOriginatorError::MalformedResponse)?;
        self.on_response_action(action)
    }

    /// Route one parsed ADDBA Response by its Dialog Token. The peer's
    /// answer, an agreement or a refusal, ends the TID's attempts.
    pub fn on_response_action(
        &mut self,
        action: BlockAckAction,
    ) -> Result<TxBlockAckResponseDisposition, TxBlockAckOriginatorError> {
        let BlockAckAction::AddbaResponse { dialog_token, .. } = action else {
            return Err(TxBlockAckOriginatorError::MalformedResponse);
        };
        let Some(agreement) = self
            .agreements
            .iter_mut()
            .flatten()
            .find(|agreement| agreement.session.awaiting_dialog_token() == Some(dialog_token))
        else {
            return Ok(TxBlockAckResponseDisposition::StaleDialogToken(
                dialog_token,
            ));
        };
        let tid = agreement.tid;
        let response = agreement
            .session
            .on_response_action(action)
            .map_err(|error| TxBlockAckOriginatorError::Session { tid, error })?;
        agreement.alarm = None;
        agreement.due = None;
        agreement.attempts = 0;
        Ok(TxBlockAckResponseDisposition::Matched(
            TxBlockAckOriginatorResponse { tid, response },
        ))
    }

    /// Consume at most one negotiation whose response is overdue at `now`
    /// and queue its next attempt after the interval while attempts remain;
    /// its TID.
    pub fn expire_next(&mut self, now: Instant) -> Option<u8> {
        let interval = self.retry_interval;
        for agreement in self.agreements.iter_mut().flatten() {
            let Some(alarm) = agreement.alarm.filter(|alarm| now >= alarm.deadline) else {
                continue;
            };
            agreement.alarm = None;
            if agreement.session.on_alarm(alarm) {
                agreement.retry(now, interval);
                return Some(agreement.tid);
            }
        }
        None
    }

    /// End the agreement or negotiation of `tid`, queueing nothing; `false`
    /// for a TID the originator does not own.
    pub fn stop(&mut self, tid: u8) -> bool {
        match self.agreement_mut(tid) {
            Some(agreement) => {
                agreement.session.stop();
                agreement.alarm = None;
                true
            }
            None => false,
        }
    }

    /// End every agreement and negotiation and drop every queued attempt;
    /// how many were operational or negotiating.
    pub fn stop_all(&mut self) -> u8 {
        let mut active = 0_u8;
        for agreement in self.agreements.iter_mut().flatten() {
            if agreement.session.operational().is_some() || agreement.alarm.is_some() {
                active = active.saturating_add(1);
            }
            agreement.session.stop();
            agreement.alarm = None;
            agreement.due = None;
            agreement.attempts = 0;
        }
        active
    }

    pub fn operational(&self, tid: u8) -> Option<OperationalTxBlockAck> {
        self.agreement(tid)?.session.operational()
    }

    /// The generation of `tid`'s session: with an operational agreement it
    /// names exactly that agreement; zero for a TID the originator does not
    /// own.
    pub fn generation(&self, tid: u8) -> u32 {
        self.agreement(tid)
            .map_or(0, |agreement| agreement.session.generation())
    }

    pub fn alarm(&self, tid: u8) -> Option<TxBlockAckAlarm> {
        self.agreement(tid)?.alarm
    }

    /// The earliest negotiation deadline.
    pub fn earliest_alarm_deadline(&self) -> Option<Instant> {
        self.agreements
            .iter()
            .flatten()
            .filter_map(|agreement| agreement.alarm)
            .map(|alarm| alarm.deadline)
            .min()
    }
}

impl Agreement {
    /// Queue the next attempt `interval` after `now`, while one remains.
    fn retry(&mut self, now: Instant, interval: Duration) {
        self.due = (self.attempts != 0).then(|| now.saturating_add(interval));
    }
}

#[cfg(test)]
mod tests;
