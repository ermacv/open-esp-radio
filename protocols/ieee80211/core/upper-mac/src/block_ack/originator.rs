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

use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee802_wire::Eui48;
use oer_protocols_ieee80211_wire::management::DialogToken;
use oer_protocols_ieee80211_wire::qos::WmmUserPriority;

use super::{
    AddbaRequest, BlockAckAction, OperationalTxBlockAck, TxBlockAckAlarm, TxBlockAckConfig,
    TxBlockAckError, TxBlockAckResponse, TxBlockAckSession, parse_block_ack_action,
};
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

/// The QoS TIDs: the most agreements one originator owns.
pub const TX_BLOCK_ACK_MAX_TIDS: usize = 8;

/// A Dialog Token sequence: it starts at `first`, and each next token is
/// one more, back to `lowest` after `highest`. A sequence is data, so the
/// originator advances it without an indirect call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DialogTokenSequence {
    /// The Dialog Token of the first request, from `lowest` to `highest`.
    pub first: DialogToken,
    pub lowest: DialogToken,
    pub highest: DialogToken,
}

impl DialogTokenSequence {
    /// From 1 through 255, skipping zero ([`DialogToken::next_nonzero`]).
    pub const NONZERO: Self = Self {
        first: DialogToken::new(1),
        lowest: DialogToken::new(1),
        highest: DialogToken::new(u8::MAX),
    };

    /// The Dialog Token after `current`.
    pub const fn next(self, current: DialogToken) -> DialogToken {
        let value = current.value();
        if value >= self.highest.value() || value < self.lowest.value() {
            self.lowest
        } else {
            DialogToken::new(value.saturating_add(1))
        }
    }

    const fn holds_first(self) -> bool {
        self.lowest.value() <= self.first.value() && self.first.value() <= self.highest.value()
    }
}

/// Which agreements an originator owns and how its Dialog Tokens advance.
#[derive(Clone, Copy, Debug)]
pub struct TxBlockAckOriginatorPolicy {
    /// The TIDs in the order their negotiations go out; at most the
    /// originator's `TIDS`.
    pub tids: &'static [WmmUserPriority],
    /// The Dialog Tokens the agreements share.
    pub dialog_tokens: DialogTokenSequence,
}

/// The agreement parameters every TID of an originator requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckOriginatorConfig {
    pub window: oer_protocols_ieee80211_wire::block_ack::frame::BlockAckWindow,
    /// How long a negotiation waits for the ADDBA Response.
    pub negotiation_timeout: Duration,
    /// The TIDs whose agreements may carry A-MSDUs.
    pub amsdu_tids: &'static [WmmUserPriority],
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
    /// The policy names this TID twice, or the originator does not own it.
    UnsupportedTid(WmmUserPriority),
    /// The policy's first Dialog Token lies outside its sequence.
    DialogTokens(DialogTokenSequence),
    MalformedResponse,
    Session {
        tid: WmmUserPriority,
        error: TxBlockAckError,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckOriginatorResponse {
    pub tid: WmmUserPriority,
    pub response: TxBlockAckResponse,
}

/// A received ADDBA Response against the negotiations the originator owns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBlockAckResponseDisposition {
    Matched(TxBlockAckOriginatorResponse),
    /// The response names no live negotiation, such as one that already
    /// timed out: it changes no agreement and does not end the link.
    StaleDialogToken(DialogToken),
}

struct Agreement {
    tid: WmmUserPriority,
    session: TxBlockAckSession,
    negotiation: Negotiation,
    /// Negotiations left; a peer's answer ends them.
    attempts: u8,
}

/// Where a TID's negotiation stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Negotiation {
    /// None waits, none is out.
    Idle,
    /// The next one is due at this instant; [`Instant::EPOCH`] at once.
    Due(Instant),
    /// One is out, its response awaited until the alarm.
    Awaiting(TxBlockAckAlarm),
    /// The peer refused the TID or its attempts ran out: [`TxBlockAckOriginator::queue`]
    /// offers it no more until [`TxBlockAckOriginator::queue_initial`] or
    /// [`TxBlockAckOriginator::stop_all`].
    Closed,
}

/// One peer's TX Block Ack agreements, at most `TIDS`.
pub struct TxBlockAckOriginator<const TIDS: usize> {
    agreements: [Option<Agreement>; TIDS],
    /// How long a failed negotiation waits before the next is due.
    retry_interval: Duration,
    next_dialog_token: DialogToken,
    dialog_tokens: DialogTokenSequence,
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
        if !policy.dialog_tokens.holds_first() {
            return Err(TxBlockAckOriginatorError::DialogTokens(
                policy.dialog_tokens,
            ));
        }
        let mut agreements = [const { None }; TIDS];
        for (index, (place, &tid)) in agreements.iter_mut().zip(policy.tids).enumerate() {
            if policy.tids.iter().take(index).any(|other| *other == tid) {
                return Err(TxBlockAckOriginatorError::UnsupportedTid(tid));
            }
            let session = TxBlockAckSession::new(TxBlockAckConfig {
                tid,
                window: config.window,
                timeout: oer_protocols_ieee80211_wire::tsf::TimeUnits::new(0),
                negotiation_timeout: config.negotiation_timeout,
                amsdu: config.amsdu_tids.contains(&tid),
            })
            .map_err(|error| TxBlockAckOriginatorError::Session { tid, error })?;
            *place = Some(Agreement {
                tid,
                session,
                negotiation: Negotiation::Idle,
                attempts: 0,
            });
        }
        Ok(Self {
            agreements,
            retry_interval: Duration::ZERO,
            next_dialog_token: policy.dialog_tokens.first,
            dialog_tokens: policy.dialog_tokens,
        })
    }

    fn agreement(&self, tid: WmmUserPriority) -> Option<&Agreement> {
        self.agreements
            .iter()
            .flatten()
            .find(|agreement| agreement.tid == tid)
    }

    fn agreement_mut(&mut self, tid: WmmUserPriority) -> Option<&mut Agreement> {
        self.agreements
            .iter_mut()
            .flatten()
            .find(|agreement| agreement.tid == tid)
    }

    /// The TIDs in the policy's order.
    pub fn tids(&self) -> impl Iterator<Item = WmmUserPriority> + '_ {
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
            agreement.attempts = retry.attempts;
            // A negotiation already out keeps its alarm.
            if !matches!(agreement.negotiation, Negotiation::Awaiting(_)) {
                agreement.negotiation = if retry.attempts == 0 {
                    Negotiation::Idle
                } else {
                    Negotiation::Due(Instant::EPOCH)
                };
            }
        }
    }

    /// Queue a negotiation of `tid` alone, due at once, with `retry`'s
    /// attempts and interval, when the TID is idle: no agreement, no
    /// negotiation, none queued and not closed by a refusal or by attempts
    /// that ran out. Whether one was queued.
    pub fn queue(&mut self, tid: WmmUserPriority, retry: TxBlockAckRetry) -> bool {
        let Some(agreement) = self.agreement_mut(tid) else {
            return false;
        };
        if agreement.negotiation != Negotiation::Idle
            || retry.attempts == 0
            || agreement.session.is_awaiting()
            || agreement.session.operational().is_some()
        {
            return false;
        }
        agreement.negotiation = Negotiation::Due(Instant::EPOCH);
        agreement.attempts = retry.attempts;
        self.retry_interval = retry.interval;
        true
    }

    /// End the agreement or negotiation of `tid` and offer it no more:
    /// [`Self::queue`] refuses it until [`Self::queue_initial`] or
    /// [`Self::stop_all`]; `false` for a TID the originator does not own.
    pub fn close(&mut self, tid: WmmUserPriority) -> bool {
        match self.agreement_mut(tid) {
            Some(agreement) => {
                agreement.session.stop();
                agreement.negotiation = Negotiation::Closed;
                agreement.attempts = 0;
                true
            }
            None => false,
        }
    }

    /// Whether `tid` is closed to [`Self::queue`].
    pub fn is_closed(&self, tid: WmmUserPriority) -> bool {
        self.agreement(tid)
            .is_some_and(|agreement| agreement.negotiation == Negotiation::Closed)
    }

    /// Whether a negotiation waits to be sent, due or not.
    pub fn has_pending(&self) -> bool {
        self.agreements
            .iter()
            .flatten()
            .any(|agreement| matches!(agreement.negotiation, Negotiation::Due(_)))
    }

    /// When the earliest waiting negotiation is due.
    pub fn next_pending_deadline(&self) -> Option<Instant> {
        self.agreements
            .iter()
            .flatten()
            .filter_map(|agreement| match agreement.negotiation {
                Negotiation::Due(due) => Some(due),
                Negotiation::Idle | Negotiation::Awaiting(_) | Negotiation::Closed => None,
            })
            .min()
    }

    /// The next TID whose negotiation is due at `now`, in the policy's
    /// order; taking it uses one of its attempts. The caller then
    /// [`Self::begin`]s it.
    pub fn take_pending(&mut self, now: Instant) -> Option<WmmUserPriority> {
        let agreement = self.agreements.iter_mut().flatten().find(
            |agreement| matches!(agreement.negotiation, Negotiation::Due(due) if due <= now),
        )?;
        agreement.negotiation = Negotiation::Idle;
        agreement.attempts = agreement.attempts.saturating_sub(1);
        Some(agreement.tid)
    }

    /// Begin the negotiation of `tid` from `starting_sequence`; the request
    /// owns its encoded action body.
    pub fn begin(
        &mut self,
        tid: WmmUserPriority,
        starting_sequence: SequenceNumber,
        now: Instant,
    ) -> Result<AddbaRequest, TxBlockAckOriginatorError> {
        let token = self.next_dialog_token;
        let agreement = self
            .agreement_mut(tid)
            .ok_or(TxBlockAckOriginatorError::UnsupportedTid(tid))?;
        let request = agreement
            .session
            .begin_with_dialog_token(starting_sequence, now, token)
            .map_err(|error| TxBlockAckOriginatorError::Session { tid, error })?;
        agreement.negotiation = Negotiation::Awaiting(request.alarm);
        self.next_dialog_token = self.dialog_tokens.next(token);
        Ok(request)
    }

    /// The request of `tid` did not leave at `now`: its negotiation ends,
    /// and the next one is due after the interval while attempts remain.
    pub fn transmit_failed(&mut self, tid: WmmUserPriority, now: Instant) {
        let interval = self.retry_interval;
        if let Some(agreement) = self.agreement_mut(tid) {
            agreement.session.stop();
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
        agreement.attempts = 0;
        agreement.negotiation = if matches!(response, TxBlockAckResponse::Rejected(_)) {
            Negotiation::Closed
        } else {
            Negotiation::Idle
        };
        Ok(TxBlockAckResponseDisposition::Matched(
            TxBlockAckOriginatorResponse { tid, response },
        ))
    }

    /// Consume at most one negotiation whose response is overdue at `now`
    /// and queue its next attempt after the interval while attempts remain;
    /// its TID.
    pub fn expire_next(&mut self, now: Instant) -> Option<WmmUserPriority> {
        let interval = self.retry_interval;
        for agreement in self.agreements.iter_mut().flatten() {
            let Negotiation::Awaiting(alarm) = agreement.negotiation else {
                continue;
            };
            if now < alarm.deadline {
                continue;
            }
            agreement.negotiation = Negotiation::Idle;
            if agreement.session.on_alarm(alarm) {
                agreement.retry(now, interval);
                return Some(agreement.tid);
            }
        }
        None
    }

    /// End the agreement or negotiation of `tid`, queueing nothing; `false`
    /// for a TID the originator does not own.
    pub fn stop(&mut self, tid: WmmUserPriority) -> bool {
        match self.agreement_mut(tid) {
            Some(agreement) => {
                agreement.session.stop();
                // A queued attempt or a closed TID stays as it is.
                if let Negotiation::Awaiting(_) = agreement.negotiation {
                    agreement.negotiation = Negotiation::Idle;
                }
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
            if agreement.session.operational().is_some()
                || matches!(agreement.negotiation, Negotiation::Awaiting(_))
            {
                active = active.saturating_add(1);
            }
            agreement.session.stop();
            agreement.negotiation = Negotiation::Idle;
            agreement.attempts = 0;
        }
        active
    }

    /// Consume delivery marks, one bit per sequence from `starting_sequence`.
    /// Advancing over deferred expiry produces one BAR at the new head.
    pub fn on_delivered(
        &mut self,
        tid: WmmUserPriority,
        starting_sequence: SequenceNumber,
        bitmap: u64,
        receiver: Eui48,
        transmitter: Eui48,
    ) -> Option<[u8; oer_protocols_ieee80211_wire::block_ack::BLOCK_ACK_REQUEST_LEN]> {
        let next = self
            .agreement_mut(tid)?
            .session
            .on_delivered(starting_sequence, bitmap)?;
        Some(
            oer_protocols_ieee80211_wire::block_ack::encode_block_ack_request(
                receiver,
                transmitter,
                tid.tid(),
                next,
            ),
        )
    }

    /// Consume expiry marks, one bit per sequence from `starting_sequence`.
    /// Non-head marks remain pending. Advancing over expiry produces one
    /// BAR at the final head, after every mark in this report was applied.
    /// Map returned aggregate positions to sequence numbers before marking;
    /// a re-formed aggregate can contain non-consecutive sequence numbers.
    /// Backends report expiry only; the core owns this request.
    pub fn on_expired(
        &mut self,
        tid: WmmUserPriority,
        starting_sequence: SequenceNumber,
        bitmap: u64,
        receiver: Eui48,
        transmitter: Eui48,
    ) -> Option<[u8; oer_protocols_ieee80211_wire::block_ack::BLOCK_ACK_REQUEST_LEN]> {
        let next = self
            .agreement_mut(tid)?
            .session
            .on_expired(starting_sequence, bitmap)?;
        Some(
            oer_protocols_ieee80211_wire::block_ack::encode_block_ack_request(
                receiver,
                transmitter,
                tid.tid(),
                next,
            ),
        )
    }

    pub fn operational(&self, tid: WmmUserPriority) -> Option<OperationalTxBlockAck> {
        self.agreement(tid)?.session.operational()
    }

    /// The generation of `tid`'s session: with an operational agreement it
    /// names exactly that agreement; zero for a TID the originator does not
    /// own.
    pub fn generation(&self, tid: WmmUserPriority) -> u32 {
        self.agreement(tid)
            .map_or(0, |agreement| agreement.session.generation())
    }

    pub fn alarm(&self, tid: WmmUserPriority) -> Option<TxBlockAckAlarm> {
        match self.agreement(tid)?.negotiation {
            Negotiation::Awaiting(alarm) => Some(alarm),
            Negotiation::Idle | Negotiation::Due(_) | Negotiation::Closed => None,
        }
    }

    /// The earliest negotiation deadline.
    pub fn earliest_alarm_deadline(&self) -> Option<Instant> {
        self.agreements
            .iter()
            .flatten()
            .filter_map(|agreement| match agreement.negotiation {
                Negotiation::Awaiting(alarm) => Some(alarm.deadline),
                Negotiation::Idle | Negotiation::Due(_) | Negotiation::Closed => None,
            })
            .min()
    }
}

impl Agreement {
    /// Queue the next attempt `interval` after `now`, while one remains.
    /// The last one closes the TID.
    fn retry(&mut self, now: Instant, interval: Duration) {
        self.negotiation = if self.attempts == 0 {
            Negotiation::Closed
        } else {
            Negotiation::Due(now.saturating_add(interval))
        };
    }
}

#[cfg(test)]
mod tests;
