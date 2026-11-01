//! The station's transmit Block Ack agreements with its access point: one
//! originator ([`TxBlockAckOriginator`]) of the eight TIDs, negotiated when
//! data of a TID comes and torn down when it stays idle.
//!
//! The composition builds [`StaTxBlockAck`] from its policy and hands it to
//! the station in its configuration; `None` negotiates nothing. The station
//! starts it afresh at each association, admits agreements only with an HT
//! access point of a QoS BSS, and owns its timing: the negotiations and the
//! response deadlines of [`TxBlockAckOriginator`], and the inactivity after
//! which an idle agreement ends with a DELBA. The access point's DELBA as
//! the recipient ends an agreement without an answer; a TID it refused, or
//! whose attempts ran out, is offered no more in that association.

use oer_ieee80211_core_upper_mac::block_ack::{
    OperationalTxBlockAck, TX_BLOCK_ACK_MAX_TIDS, TxBlockAckConfig, TxBlockAckOriginator,
    TxBlockAckOriginatorConfig, TxBlockAckOriginatorError, TxBlockAckOriginatorPolicy,
    TxBlockAckResponse, TxBlockAckResponseDisposition, TxBlockAckRetry, next_nonzero_dialog_token,
};
use oer_ieee80211_wire::block_ack::frame::BlockAckAction;
use oer_time::{Duration, Instant};

/// Every QoS TID, in the order their negotiations go out.
const ALL_TIDS: [u8; TX_BLOCK_ACK_MAX_TIDS] = [0, 1, 2, 3, 4, 5, 6, 7];

/// The composition's policy for the station's transmit agreements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTxBlockAckPolicy {
    /// The window the ADDBA Request asks for: at most what the port's
    /// aggregates carry, 1 to 64.
    pub window: u16,
    /// How long a negotiation waits for the ADDBA Response.
    pub negotiation_timeout: Duration,
    /// How many requests one negotiation makes and how far apart.
    pub retry: TxBlockAckRetry,
    /// How long an agreement may carry no data before the station ends it.
    pub inactivity: Duration,
    /// How long an aggregate's missing member is retried.
    pub retry_members: crate::data::AggregateRetry,
}

/// Why a policy builds no originator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTxBlockAckPolicyError {
    /// The window or negotiation timeout is outside the agreement's range.
    Agreement(TxBlockAckOriginatorError),
    /// A negotiation would make no request.
    NoAttempts,
    /// An idle agreement would end at once.
    ZeroInactivity,
    /// A missing aggregate member would never be retried.
    NoMemberRetry,
}

/// What the station's agreements owe the access point now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TxBlockAckOwed {
    /// A DELBA, as the originator, ending `tid`'s agreement.
    Delba { tid: u8 },
    /// An ADDBA Request negotiating `tid`, which the caller begins.
    Negotiate { tid: u8 },
    /// A BlockAckReq moving the recipient's window of `tid` to `start`.
    Request {
        tid: u8,
        start: oer_ieee80211_wire::sequence::SequenceNumber,
    },
}

/// The station's transmit Block Ack originator and its per-association state.
pub struct StaTxBlockAck {
    originator: TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS>,
    retry: TxBlockAckRetry,
    inactivity: Duration,
    retry_members: crate::data::AggregateRetry,
    /// When data of each TID last came while the association admits agreements.
    traffic: [Option<Instant>; TX_BLOCK_ACK_MAX_TIDS],
    /// TIDs whose ended agreement owes the access point a DELBA, one bit each.
    delba: u8,
    /// Where each TID's owed BlockAckReq moves the recipient's window.
    request: [Option<oer_ieee80211_wire::sequence::SequenceNumber>; TX_BLOCK_ACK_MAX_TIDS],
    /// The current association admits agreements: an HT access point of a QoS BSS.
    admitted: bool,
    /// The earliest instant the agreements' timing (a response deadline, an
    /// inactivity end) may change, so [`Self::poll`] scans it only then;
    /// `None` while none is set. Every change that may set an earlier one
    /// moves it earlier.
    timing: Option<Instant>,
}

impl StaTxBlockAck {
    /// The originator of `policy`, with no association.
    ///
    /// # Errors
    ///
    /// The policy's window or timeout is outside the agreement's range, it
    /// allows no request, or its inactivity is zero.
    pub fn new(policy: StaTxBlockAckPolicy) -> Result<Self, StaTxBlockAckPolicyError> {
        if policy.retry.attempts == 0 {
            return Err(StaTxBlockAckPolicyError::NoAttempts);
        }
        if policy.inactivity == Duration::ZERO {
            return Err(StaTxBlockAckPolicyError::ZeroInactivity);
        }
        if policy.retry_members.attempts == 0 || policy.retry_members.lifetime == Duration::ZERO {
            return Err(StaTxBlockAckPolicyError::NoMemberRetry);
        }
        // Validate before building, so the error names the policy's field.
        TxBlockAckConfig {
            tid: 0,
            window: policy.window,
            timeout_tu: 0,
            negotiation_timeout: policy.negotiation_timeout,
            amsdu: false,
        }
        .validate()
        .map_err(|error| {
            StaTxBlockAckPolicyError::Agreement(TxBlockAckOriginatorError::Session {
                tid: 0,
                error,
            })
        })?;
        let originator = TxBlockAckOriginator::new(
            TxBlockAckOriginatorPolicy {
                tids: &ALL_TIDS,
                first_dialog_token: 1,
                next_dialog_token: next_nonzero_dialog_token,
            },
            TxBlockAckOriginatorConfig {
                window: policy.window,
                negotiation_timeout: policy.negotiation_timeout,
                amsdu_tids: 0,
            },
        )
        .map_err(StaTxBlockAckPolicyError::Agreement)?;
        Ok(Self {
            originator,
            retry: policy.retry,
            inactivity: policy.inactivity,
            retry_members: policy.retry_members,
            traffic: [None; TX_BLOCK_ACK_MAX_TIDS],
            delba: 0,
            request: [None; TX_BLOCK_ACK_MAX_TIDS],
            admitted: false,
            timing: None,
        })
    }

    /// A new association: every agreement of the previous one is gone with
    /// it, and none is owed a DELBA. `admitted` is whether this one admits
    /// agreements.
    pub(crate) fn associate(&mut self, admitted: bool) {
        self.originator.stop_all();
        self.traffic = [None; TX_BLOCK_ACK_MAX_TIDS];
        self.delba = 0;
        self.request = [None; TX_BLOCK_ACK_MAX_TIDS];
        self.admitted = admitted;
        self.retime();
    }

    /// Take the timing again after a change of the agreements.
    fn retime(&mut self) {
        self.timing = self.next_timing();
    }

    /// The earliest response deadline or inactivity end.
    fn next_timing(&self) -> Option<Instant> {
        let idle = ALL_TIDS
            .into_iter()
            .filter_map(|tid| self.idle_until(tid))
            .min();
        [self.originator.earliest_alarm_deadline(), idle]
            .into_iter()
            .flatten()
            .min()
    }

    /// One terminal settled `marks` of a TID's sequence numbers: the
    /// agreement's window moves over them, and when it passed a dropped one
    /// the recipient owes a BlockAckReq to move its own. `receiver` and
    /// `transmitter` name the agreement's peers.
    pub(crate) fn settle(
        &mut self,
        marks: crate::data::BlockAckMarks,
        receiver: [u8; 6],
        transmitter: [u8; 6],
    ) {
        let tid = marks.tid.value();
        if !self.admitted || self.originator.operational(tid).is_none() {
            return;
        }
        let delivered = self
            .originator
            .on_delivered(tid, marks.start, marks.delivered, receiver, transmitter)
            .is_some();
        let expired = self
            .originator
            .on_expired(tid, marks.start, marks.expired, receiver, transmitter)
            .is_some();
        if (delivered || expired)
            && let Some(agreement) = self.originator.operational(tid)
            && let Some(request) = self.request.get_mut(usize::from(tid))
        {
            *request = Some(agreement.starting_sequence);
        }
    }

    /// The BlockAckReq of `tid` went to the port.
    pub(crate) fn request_sent(&mut self, tid: u8) {
        if let Some(request) = self.request.get_mut(usize::from(tid)) {
            *request = None;
        }
    }

    /// The BlockAckReq of `tid` went unanswered: it is owed again at the
    /// agreement's current head while the agreement lives.
    pub(crate) fn request_unanswered(&mut self, tid: u8) {
        let head = self
            .originator
            .operational(tid)
            .map(|agreement| agreement.starting_sequence);
        if let Some(request) = self.request.get_mut(usize::from(tid))
            && request.is_none()
        {
            *request = head;
        }
    }

    /// Data of `tid` came at `now`: an idle TID negotiates, a live one stays
    /// active.
    pub(crate) fn traffic(&mut self, tid: u8, now: Instant) {
        if !self.admitted {
            return;
        }
        let Some(last) = self.traffic.get_mut(usize::from(tid)) else {
            return;
        };
        *last = Some(now);
        // An operational agreement's inactivity ends no earlier than this.
        if self.originator.operational(tid).is_some() {
            let idle = now.saturating_add(self.inactivity);
            self.timing = Some(self.timing.map_or(idle, |at| at.min(idle)));
        }
        // A live, queued or closed TID refuses; that is its normal state.
        let _queued: bool = self.originator.queue(tid, self.retry);
    }

    /// How long an aggregate's missing member is retried.
    pub(crate) const fn retry_members(&self) -> crate::data::AggregateRetry {
        self.retry_members
    }

    /// The operational agreement of `tid`.
    pub(crate) fn operational(&self, tid: u8) -> Option<OperationalTxBlockAck> {
        if !self.admitted {
            return None;
        }
        self.originator.operational(tid)
    }

    /// Advance the agreements' timing to `now`: overdue responses end their
    /// negotiations, idle agreements end and owe a DELBA. Then what is owed:
    /// a DELBA first, then a due negotiation, which takes one of its
    /// attempts. `can_send` is false while the station has no transmission
    /// to give; nothing is taken then.
    pub(crate) fn poll(&mut self, now: Instant, can_send: bool) -> Option<TxBlockAckOwed> {
        if self.timing.is_some_and(|at| now >= at) {
            // At most one expiry per agreement.
            for _ in ALL_TIDS {
                if self.originator.expire_next(now).is_none() {
                    break;
                }
            }
            for tid in ALL_TIDS {
                if self.idle_until(tid).is_some_and(|until| now >= until)
                    && self.originator.stop(tid)
                    && let Some(bit) = 1_u8.checked_shl(u32::from(tid))
                {
                    self.delba |= bit;
                }
            }
            self.timing = self.next_timing();
        }
        if !can_send {
            return None;
        }
        for tid in ALL_TIDS {
            let start = self.request.get(usize::from(tid)).copied().flatten();
            if let Some(start) = start {
                if self.originator.operational(tid).is_some() {
                    return Some(TxBlockAckOwed::Request { tid, start });
                }
                self.request_sent(tid);
            }
        }
        if self.delba != 0 {
            // trailing_zeros of a nonzero u8 is below eight.
            let tid = self.delba.trailing_zeros() as u8;
            return Some(TxBlockAckOwed::Delba { tid });
        }
        self.originator
            .take_pending(now)
            .map(|tid| TxBlockAckOwed::Negotiate { tid })
    }

    /// The DELBA of `tid` went to the port.
    pub(crate) fn delba_sent(&mut self, tid: u8) {
        if let Some(bit) = 1_u8.checked_shl(u32::from(tid)) {
            self.delba &= !bit;
        }
    }

    /// Begin `tid`'s negotiation from `starting_sequence`.
    pub(crate) fn begin(
        &mut self,
        tid: u8,
        starting_sequence: oer_ieee80211_wire::sequence::SequenceNumber,
        now: Instant,
    ) -> Result<oer_ieee80211_core_upper_mac::block_ack::AddbaRequest, TxBlockAckOriginatorError>
    {
        let request = self.originator.begin(tid, starting_sequence, now);
        self.retime();
        request
    }

    /// `tid`'s request did not leave: its next attempt is due after the
    /// retry interval while attempts remain.
    pub(crate) fn request_failed(&mut self, tid: u8, now: Instant) {
        self.originator.transmit_failed(tid, now);
        self.retime();
    }

    /// Take the access point's answer to the station's agreements: an ADDBA
    /// Response, or its DELBA as the recipient. `false` for every other
    /// action, which belongs to the receive agreements. A response that
    /// breaks the agreement's rules ends the TID's negotiation, closes it
    /// and owes a DELBA, so the access point keeps no agreement of its own.
    pub(crate) fn on_action(&mut self, action: BlockAckAction, now: Instant) -> bool {
        let taken = self.take_action(action, now);
        self.retime();
        taken
    }

    fn take_action(&mut self, action: BlockAckAction, now: Instant) -> bool {
        match action {
            BlockAckAction::AddbaResponse { tid, .. } => {
                match self.originator.on_response_action(action) {
                    Ok(TxBlockAckResponseDisposition::Matched(matched)) => {
                        if let TxBlockAckResponse::Operational(agreement) = matched.response
                            && let Some(last) = self.traffic.get_mut(usize::from(agreement.tid))
                        {
                            // The agreement is idle from its start.
                            *last = Some(now);
                        }
                    }
                    // A token that owns no live negotiation is stale.
                    Ok(TxBlockAckResponseDisposition::StaleDialogToken(_)) => {}
                    Err(TxBlockAckOriginatorError::Session { tid: session, .. }) => {
                        self.refuse(session);
                    }
                    Err(_) => self.refuse(tid),
                }
                true
            }
            BlockAckAction::Delba {
                tid,
                initiator: false,
                ..
            } => {
                self.originator.stop(tid);
                true
            }
            BlockAckAction::AddbaRequest { .. } | BlockAckAction::Delba { .. } => false,
        }
    }

    /// The earliest instant the agreements need [`Self::poll`]: a response
    /// deadline, an agreement's inactivity end, and while `can_send` a due
    /// negotiation or an owed DELBA.
    pub(crate) fn deadline(&self, can_send: bool) -> Option<Instant> {
        let due = if !can_send {
            None
        } else if self.delba != 0 || self.request.iter().any(Option::is_some) {
            Some(Instant::EPOCH)
        } else {
            self.originator.next_pending_deadline()
        };
        [self.timing, due].into_iter().flatten().min()
    }

    /// When `tid`'s operational agreement ends for inactivity.
    fn idle_until(&self, tid: u8) -> Option<Instant> {
        self.originator.operational(tid)?;
        let last = (*self.traffic.get(usize::from(tid))?)?;
        Some(last.saturating_add(self.inactivity))
    }

    /// End and close `tid` after a response the station cannot accept.
    fn refuse(&mut self, tid: u8) {
        if self.originator.close(tid)
            && let Some(bit) = 1_u8.checked_shl(u32::from(tid))
        {
            self.delba |= bit;
        }
    }
}

#[cfg(test)]
mod tests;
