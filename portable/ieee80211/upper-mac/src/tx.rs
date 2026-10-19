//! The transmit planner: one frame exchange as a sequence of lower-MAC
//! attempts.
//!
//! A lower-MAC attempt is one hardware transmission. [`TxPlanner`] turns an
//! outbound MPDU or A-MPDU into those attempts. [`TxPlanner::begin`] returns
//! the exchange's state ([`TxExchange`]) and its first [`TxAttemptPlan`]: the
//! rate the peer's [`RateLadder`] gives, the protection exchange the
//! [`ProtectionPolicy`] selects, and which frame content with which Retry
//! bits the attempt carries. It draws no backoff: the lower-MAC port carries
//! none, and a backend draws its queues' contention below the port. [`TxPlanner::complete`]
//! takes that attempt's [`PublicationReport`] and returns either the next plan or
//! the exchange's [`TxReport`]:
//!
//! - an MPDU follows [`MpduRetryState`]: retried at the ladder's next rate
//!   until delivered or a retry limit is reached;
//! - an A-MPDU follows [`AmpduRetryState`]: retransmitted as an aggregate of
//!   the unacknowledged subframes, republished unchanged after a failed
//!   protection exchange, asked for its BlockAck with a BlockAckReq, and
//!   finished or handed over to individual transmission of its missing
//!   subframes, each again an MPDU exchange.
//!
//! The planner never holds frame bytes and never waits: the caller encodes
//! each attempt from its own copy of the frames, submits it and feeds the
//! completion back. A retransmission carries the bytes first encoded, so
//! its sequence number and CCMP packet number repeat.

use oer_ieee80211_lower_mac::Ieee80211Instant;
use oer_ieee80211_lower_mac::{BlockAckReport, CoexPriority, TxPower, TxResponse, TxStatus};
use oer_ieee80211_mac::{phy::PhyRate, qos::WmmAccessCategory, sequence::SequenceNumber};
use oer_ieee80211_softmac::{
    MacAmpduTxResult, MacAmpduTxStatus, MacIndividualRetries, MacTxResult, MacTxStatus,
};

use crate::{
    ampdu::{
        AmpduAttemptResult, AmpduRetryDecision, AmpduRetryError, AmpduRetryPolicy, AmpduRetryState,
        MAX_AMPDU_SUBFRAMES,
    },
    protection::{
        HeTxopRtsBudget, ProtectedPpdu, ProtectionPolicy, TxProtection, TxProtectionDecision,
        TxPsdu, TxReceiver,
    },
    retry::{
        FrameClass, MpduRetryState, RateLadder, RetryDecision, RetryLimits, RetryOutcome,
        RetryStateError,
    },
};

/// One publication observed by this planner, before it decides whether
/// its exchange continues. The port's terminal `TxOutcome` is separate:
/// retry observations must not be mistaken for resource-return terminals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicationReport {
    /// The publication's hardware result.
    pub status: TxStatus,
    /// The solicited ACK/BlockAck's SNR.
    pub ack_snr_db: Option<i8>,
    /// The BlockAck received by this publication.
    pub block_ack: Option<BlockAckReport>,
}

/// Octets an A-MPDU subframe adds before its MPDU: the MPDU delimiter.
const AMPDU_DELIMITER: u32 = 4;

/// One outbound MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MpduRequest {
    /// Octets on air, MAC header to FCS, MIC included.
    pub length: u32,
    /// The response the MPDU solicits: `Ack` for an individually addressed
    /// frame, `None` for a group-addressed one.
    pub response: TxResponse,
}

/// One outbound A-MPDU of consecutive sequence numbers under one TID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduRequest {
    pub tid: u8,
    /// The sequence number of subframe zero; subframe `i` carries
    /// `first_sequence + i`.
    pub first_sequence: SequenceNumber,
    subframes: u8,
    lengths: [u16; MAX_AMPDU_SUBFRAMES as usize],
    /// When the MSDUs were committed: the aggregate's lifetime runs from
    /// here, on the backend's radio clock.
    pub committed_at: Ieee80211Instant,
    /// Whether the Block Ack agreement is operational; while it is not,
    /// missing subframes leave the aggregate.
    pub block_ack_operational: bool,
}

impl AmpduRequest {
    /// An aggregate of subframes whose MPDUs are `lengths` octets on air
    /// (MAC header to FCS, MIC included); `None` for no subframe or more
    /// than [`MAX_AMPDU_SUBFRAMES`].
    pub fn new(
        tid: u8,
        first_sequence: SequenceNumber,
        lengths: &[u16],
        committed_at: Ieee80211Instant,
        block_ack_operational: bool,
    ) -> Option<Self> {
        if lengths.is_empty() || lengths.len() > usize::from(MAX_AMPDU_SUBFRAMES) {
            return None;
        }
        let mut owned = [0; MAX_AMPDU_SUBFRAMES as usize];
        for (to, from) in owned.iter_mut().zip(lengths) {
            *to = *from;
        }
        Some(Self {
            tid,
            first_sequence,
            subframes: lengths.len() as u8,
            lengths: owned,
            committed_at,
            block_ack_operational,
        })
    }

    pub const fn subframes(&self) -> u8 {
        self.subframes
    }

    /// The sequence number of subframe `index`.
    pub const fn sequence(&self, index: u8) -> SequenceNumber {
        self.first_sequence.wrapping_add(index as u16)
    }

    /// Octets on air of subframe `index`'s MPDU.
    pub fn mpdu_length(&self, index: u8) -> Option<u32> {
        self.lengths
            .iter()
            .take(usize::from(self.subframes))
            .nth(usize::from(index))
            .copied()
            .map(u32::from)
    }

    /// The A-MPDU of the subframes of `mask`, in order: each MPDU after its
    /// delimiter, padded to four octets except the last.
    pub fn aggregate_length(&self, mask: u64) -> u32 {
        let selected = self
            .lengths
            .iter()
            .take(usize::from(self.subframes))
            .enumerate()
            .filter(|(index, _)| mask & (1_u64 << index) != 0);
        let mut selected = selected.peekable();
        let mut length = 0_u32;
        while let Some((_, mpdu_length)) = selected.next() {
            let subframe = AMPDU_DELIMITER.saturating_add(u32::from(*mpdu_length));
            length = length.saturating_add(if selected.peek().is_none() {
                subframe
            } else {
                subframe.next_multiple_of(4)
            });
        }
        length
    }
}

/// What an exchange sends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBody {
    Mpdu(MpduRequest),
    Ampdu(AmpduRequest),
}

/// One outbound frame exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxRequest {
    pub access_category: WmmAccessCategory,
    /// The rate the peer's rate control chose for the first attempt.
    pub initial_rate: PhyRate,
    pub receiver: TxReceiver,
    pub power: TxPower,
    pub coex: CoexPriority,
    /// Unacknowledged transmissions one MPDU of the exchange may make.
    pub mpdu_retry_limit: u8,
    pub body: TxBody,
}

/// Immutable fields shared by every phase of one exchange. The body stays
/// solely in the phase whose state proves whether it is an MPDU or A-MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RequestContext {
    access_category: WmmAccessCategory,
    initial_rate: PhyRate,
    receiver: TxReceiver,
    power: TxPower,
    coex: CoexPriority,
    mpdu_retry_limit: u8,
}

impl RequestContext {
    const fn from_request(request: &TxRequest) -> Self {
        Self {
            access_category: request.access_category,
            initial_rate: request.initial_rate,
            receiver: request.receiver,
            power: request.power,
            coex: request.coex,
            mpdu_retry_limit: request.mpdu_retry_limit,
        }
    }

    const fn request(self, body: TxBody) -> TxRequest {
        TxRequest {
            access_category: self.access_category,
            initial_rate: self.initial_rate,
            receiver: self.receiver,
            power: self.power,
            coex: self.coex,
            mpdu_retry_limit: self.mpdu_retry_limit,
            body,
        }
    }
}

/// The frame content of one attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptContent {
    /// The request's MPDU; with its Retry bit set once it was on air.
    Mpdu { set_retry_bit: bool },
    /// An A-MPDU of the request's subframes in `subframes` (bit `i` is
    /// subframe `i`), in order; those in `retry` carry the Retry bit.
    Ampdu { subframes: u64, retry: u64 },
    /// Subframe `index` of the request alone, answered by an ACK, with its
    /// Retry bit set: it was on air in an aggregate.
    Subframe { index: u8 },
    /// A BlockAckReq for the aggregate's TID, starting at
    /// `starting_sequence`, answered by a BlockAck.
    BlockAckRequest {
        tid: u8,
        starting_sequence: SequenceNumber,
    },
}

/// Everything one lower-MAC attempt needs besides its frame bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxAttemptPlan {
    pub access_category: WmmAccessCategory,
    pub rate: PhyRate,
    pub protection: TxProtectionDecision,
    pub power: TxPower,
    pub coex: CoexPriority,
    pub content: AttemptContent,
}

/// The statistics of a finished exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxReport {
    Mpdu(MacTxStatus),
    Ampdu(MacAmpduTxStatus),
}

/// The next step of an exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxStep {
    /// Submit this attempt and feed its completion back.
    Attempt(TxAttemptPlan),
    /// The exchange ended.
    Done(TxReport),
}

/// Why an exchange could not begin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxPlanError {
    Retry(RetryStateError),
    Ampdu(AmpduRetryError),
    /// A requested original subframe is outside this aggregate.
    SubframeOutsideAggregate,
}

/// The exchange's phase: running, or finished with its report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Live(Live),
    Done(TxReport, TxBody),
}

/// A running exchange's phase: each holds the request's body of its own
/// kind, so a phase never meets a body it does not send.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Live {
    Mpdu(MpduRetryState, MpduRequest),
    Aggregate(AmpduRetryState, AmpduRequest),
    BlockAckRequest(AmpduRetryState, AmpduRequest),
    Individual {
        aggregate: AmpduRetryState,
        /// Original subframes still to send, the current one included.
        remaining: u64,
        retry: MpduRetryState,
        ampdu: AmpduRequest,
        /// Checked once when this individual retry is selected.
        length: u32,
    },
}

/// The state of one frame exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxExchange {
    context: RequestContext,
    phase: Phase,
    /// Original subframes (bit zero for an MPDU) that were on air.
    transmitted: u64,
    /// The rate of the last attempt.
    rate: PhyRate,
    /// The aggregate of the last A-MPDU attempt.
    aggregate: u64,
    aggregate_attempts: u8,
    individual: MacIndividualRetries,
    ack_snr_db: Option<i8>,
    /// Whether the last A-MPDU attempt ended without any BlockAck.
    unanswered: bool,
}

impl TxExchange {
    /// Reconstruct the request as a value from its common context and the
    /// body held by the current phase.
    ///
    /// ```compile_fail,E0308
    /// use oer_ieee80211_upper_mac::{TxExchange, TxRequest};
    /// fn borrowed_request(exchange: &TxExchange) -> &TxRequest {
    ///     exchange.request()
    /// }
    /// ```
    ///
    /// ```
    /// use oer_ieee80211_upper_mac::{TxExchange, TxRequest};
    /// fn request_snapshot(exchange: &TxExchange) -> TxRequest {
    ///     exchange.request()
    /// }
    /// ```
    pub const fn request(&self) -> TxRequest {
        self.context.request(self.body())
    }

    const fn body(&self) -> TxBody {
        match self.phase {
            Phase::Live(Live::Mpdu(_, mpdu)) => TxBody::Mpdu(mpdu),
            Phase::Live(
                Live::Aggregate(_, ampdu)
                | Live::BlockAckRequest(_, ampdu)
                | Live::Individual { ampdu, .. },
            ) => TxBody::Ampdu(ampdu),
            Phase::Done(_, body) => body,
        }
    }

    pub const fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done(..))
    }

    /// The report of a finished exchange.
    pub const fn report(&self) -> Option<TxReport> {
        match self.phase {
            Phase::Done(report, _) => Some(report),
            _ => None,
        }
    }

    /// Mark whether the Block Ack agreement is operational; the next
    /// completion of the aggregate reads it.
    pub fn set_block_ack_operational(&mut self, operational: bool) {
        match &mut self.phase {
            Phase::Live(
                Live::Aggregate(_, ampdu)
                | Live::BlockAckRequest(_, ampdu)
                | Live::Individual { ampdu, .. },
            )
            | Phase::Done(_, TxBody::Ampdu(ampdu)) => {
                ampdu.block_ack_operational = operational;
            }
            Phase::Live(Live::Mpdu(..)) | Phase::Done(_, TxBody::Mpdu(_)) => {}
        }
    }
}

/// The transmit planner of one interface: the retry and protection policies
/// every exchange follows.
pub struct TxPlanner<B = crate::protection::ProtectEveryHeTxop> {
    retry_limits: RetryLimits,
    ampdu_policy: AmpduRetryPolicy,
    protection: ProtectionPolicy,
    he_budget: B,
}

impl<B: HeTxopRtsBudget> TxPlanner<B> {
    /// A planner with these retry limits and policies.
    pub const fn new(
        retry_limits: RetryLimits,
        ampdu_policy: AmpduRetryPolicy,
        protection: ProtectionPolicy,
        he_budget: B,
    ) -> Self {
        Self {
            retry_limits,
            ampdu_policy,
            protection,
            he_budget,
        }
    }

    pub fn protection(&self) -> &ProtectionPolicy {
        &self.protection
    }

    pub fn protection_mut(&mut self) -> &mut ProtectionPolicy {
        &mut self.protection
    }

    pub const fn ampdu_policy(&self) -> AmpduRetryPolicy {
        self.ampdu_policy
    }

    pub fn set_ampdu_policy(&mut self, policy: AmpduRetryPolicy) {
        self.ampdu_policy = policy;
    }

    /// Start an exchange and plan its first attempt.
    pub fn begin(
        &mut self,
        request: TxRequest,
        ladder: &impl RateLadder,
    ) -> Result<(TxExchange, TxAttemptPlan), TxPlanError> {
        let context = RequestContext::from_request(&request);
        let phase = match request.body {
            TxBody::Mpdu(mpdu) => Live::Mpdu(self.mpdu_retry(&context, mpdu.length)?, mpdu),
            TxBody::Ampdu(ampdu) => Live::Aggregate(
                AmpduRetryState::new(
                    ampdu.first_sequence,
                    ampdu.subframes,
                    self.ampdu_policy,
                    ampdu.committed_at,
                )
                .map_err(TxPlanError::Ampdu)?,
                ampdu,
            ),
        };
        let mut exchange = TxExchange {
            context,
            phase: Phase::Live(phase),
            transmitted: 0,
            rate: request.initial_rate,
            aggregate: 0,
            aggregate_attempts: 0,
            individual: MacIndividualRetries::NONE,
            ack_snr_db: None,
            unanswered: false,
        };
        let plan = self.plan(&mut exchange, phase, ladder);
        Ok((exchange, plan))
    }

    /// Apply the completion of the exchange's last attempt, observed at
    /// `now` on the backend's radio clock, and plan the next step.
    ///
    /// A completion after the exchange ended returns its report again.
    pub fn complete(
        &mut self,
        exchange: &mut TxExchange,
        completion: &PublicationReport,
        now: Ieee80211Instant,
        ladder: &impl RateLadder,
    ) -> TxStep {
        if completion.ack_snr_db.is_some() {
            exchange.ack_snr_db = completion.ack_snr_db;
        }
        let live = match exchange.phase {
            Phase::Done(report, _) => return TxStep::Done(report),
            Phase::Live(live) => live,
        };
        let next = match live {
            Live::Mpdu(mut retry, mpdu) => match retry.observe(completion.status) {
                RetryDecision::Retry { set_retry_bit } => {
                    if set_retry_bit {
                        exchange.transmitted = 1;
                    }
                    Live::Mpdu(retry, mpdu)
                }
                RetryDecision::Complete(outcome) => {
                    let report = TxReport::Mpdu(Self::mpdu_status(exchange, &retry, outcome));
                    exchange.phase = Phase::Done(report, exchange.body());
                    return TxStep::Done(report);
                }
            },
            Live::Aggregate(mut state, ampdu) => {
                let result = match (completion.block_ack, completion.status) {
                    (Some(report), _) => AmpduAttemptResult::Answered(Some(report)),
                    (None, TxStatus::Success) => AmpduAttemptResult::Answered(None),
                    (None, TxStatus::CtsTimeout) => AmpduAttemptResult::ProtectionFailure,
                    (None, TxStatus::AckTimeout | TxStatus::Collision) => {
                        AmpduAttemptResult::NoResponse
                    }
                    (None, TxStatus::TimedOut) => {
                        return self.finish_ampdu(exchange, &state, MacAmpduTxResult::Incomplete);
                    }
                };
                if !matches!(result, AmpduAttemptResult::ProtectionFailure) {
                    exchange.transmitted |= exchange.aggregate;
                }
                exchange.unanswered = matches!(result, AmpduAttemptResult::NoResponse);
                let current = state.current_subframes();
                let operational = self.block_ack_operational(exchange);
                match state.observe(result, current, now, operational) {
                    Ok(decision) => match self.after_aggregate(exchange, state, ampdu, decision) {
                        Some(phase) => phase,
                        None => return self.finished(exchange, &state),
                    },
                    Err(_) => {
                        return self.finish_ampdu(exchange, &state, MacAmpduTxResult::Incomplete);
                    }
                }
            }
            Live::BlockAckRequest(mut state, ampdu) => {
                let report = match completion.status {
                    TxStatus::Success => completion.block_ack,
                    _ => None,
                };
                exchange.unanswered = false;
                let operational = self.block_ack_operational(exchange);
                let decision = state.observe_block_ack_request(report, now, operational);
                match self.after_aggregate(exchange, state, ampdu, decision) {
                    Some(phase) => phase,
                    None => return self.finished(exchange, &state),
                }
            }
            Live::Individual {
                aggregate,
                mut remaining,
                mut retry,
                ampdu,
                length,
            } => match retry.observe(completion.status) {
                RetryDecision::Retry { .. } => Live::Individual {
                    aggregate,
                    remaining,
                    retry,
                    ampdu,
                    length,
                },
                RetryDecision::Complete(outcome) => {
                    let status = Self::mpdu_status(exchange, &retry, outcome);
                    exchange.individual.record(status);
                    remaining &= remaining.wrapping_sub(1);
                    if remaining == 0 {
                        let result = if exchange.individual.failed == 0 {
                            MacAmpduTxResult::Delivered
                        } else {
                            MacAmpduTxResult::Incomplete
                        };
                        return self.finish_ampdu(exchange, &aggregate, result);
                    }
                    let index = remaining.trailing_zeros() as u8;
                    match self.subframe_retry(exchange, index) {
                        Ok((retry, length)) => Live::Individual {
                            aggregate,
                            remaining,
                            retry,
                            ampdu,
                            length,
                        },
                        Err(_) => {
                            return self.finish_ampdu(
                                exchange,
                                &aggregate,
                                MacAmpduTxResult::Incomplete,
                            );
                        }
                    }
                }
            },
        };
        exchange.phase = Phase::Live(next);
        TxStep::Attempt(self.plan(exchange, next, ladder))
    }

    /// The phase after an aggregate decision; `None` when the exchange ends.
    fn after_aggregate(
        &mut self,
        exchange: &mut TxExchange,
        state: AmpduRetryState,
        ampdu: AmpduRequest,
        decision: AmpduRetryDecision,
    ) -> Option<Live> {
        match decision {
            AmpduRetryDecision::RetainAggregate { .. }
            | AmpduRetryDecision::RepublishUnchanged { .. } => Some(Live::Aggregate(state, ampdu)),
            AmpduRetryDecision::RequestBlockAck { .. } => {
                // The exhausted protection retries end this frame exchange
                // sequence; the BlockAckReq starts the next one.
                Some(Live::BlockAckRequest(state, ampdu))
            }
            AmpduRetryDecision::Unaggregate { retry_mask } => {
                let mut remaining = 0_u64;
                for position in 0..state.current_subframes() {
                    if retry_mask & (1 << position) != 0 {
                        remaining |= 1 << state.original_index(position);
                    }
                }
                let index = remaining.trailing_zeros() as u8;
                match self.subframe_retry(exchange, index) {
                    Ok((retry, length)) => Some(Live::Individual {
                        aggregate: state,
                        remaining,
                        retry,
                        ampdu,
                        length,
                    }),
                    Err(_) => None,
                }
            }
            AmpduRetryDecision::Finish { .. } | AmpduRetryDecision::FinishTriggerFlow => None,
        }
    }

    fn finished(&self, exchange: &mut TxExchange, state: &AmpduRetryState) -> TxStep {
        let result = if state.acknowledged() == self.original_subframes(exchange) {
            MacAmpduTxResult::Delivered
        } else if exchange.unanswered {
            MacAmpduTxResult::HardwareTimeout
        } else {
            MacAmpduTxResult::Incomplete
        };
        self.finish_ampdu(exchange, state, result)
    }

    fn finish_ampdu(
        &self,
        exchange: &mut TxExchange,
        state: &AmpduRetryState,
        result: MacAmpduTxResult,
    ) -> TxStep {
        let report = TxReport::Ampdu(MacAmpduTxStatus {
            result,
            original_subframes: u16::from(self.original_subframes(exchange)),
            aggregate_attempts: exchange.aggregate_attempts,
            aggregate_rate: exchange.context.initial_rate,
            block_acknowledged_subframes: u16::from(state.acknowledged()),
            ack_snr_db: if state.acknowledged() > 0 {
                exchange.ack_snr_db
            } else {
                None
            },
            individual_retries: exchange.individual,
        });
        exchange.phase = Phase::Done(report, exchange.body());
        TxStep::Done(report)
    }

    fn original_subframes(&self, exchange: &TxExchange) -> u8 {
        match exchange.body() {
            TxBody::Mpdu(_) => 1,
            TxBody::Ampdu(ampdu) => ampdu.subframes,
        }
    }

    fn block_ack_operational(&self, exchange: &TxExchange) -> bool {
        match exchange.body() {
            TxBody::Ampdu(ampdu) => ampdu.block_ack_operational,
            TxBody::Mpdu(_) => false,
        }
    }

    fn mpdu_retry(
        &self,
        request: &RequestContext,
        length: u32,
    ) -> Result<MpduRetryState, TxPlanError> {
        let frame_class = match self.protection.rts_length_threshold() {
            Some(threshold) if length > u32::from(threshold.bytes()) => FrameClass::Long,
            _ => FrameClass::Short,
        };
        MpduRetryState::new(self.retry_limits, request.mpdu_retry_limit, frame_class)
            .map_err(TxPlanError::Retry)
    }

    fn subframe_retry(
        &self,
        exchange: &TxExchange,
        index: u8,
    ) -> Result<(MpduRetryState, u32), TxPlanError> {
        let length = match exchange.body() {
            TxBody::Ampdu(ampdu) => ampdu
                .mpdu_length(index)
                .ok_or(TxPlanError::SubframeOutsideAggregate)?,
            TxBody::Mpdu(mpdu) => mpdu.length,
        };
        self.mpdu_retry(&exchange.context, length)
            .map(|retry| (retry, length))
    }

    fn mpdu_status(
        exchange: &TxExchange,
        retry: &MpduRetryState,
        outcome: RetryOutcome,
    ) -> MacTxStatus {
        let solicits_ack = match exchange.body() {
            TxBody::Mpdu(mpdu) => mpdu.response != TxResponse::None,
            TxBody::Ampdu(_) => true,
        };
        let (result, delivered) = match outcome {
            RetryOutcome::Delivered => (MacTxResult::Transmitted, true),
            RetryOutcome::RetryLimit(TxStatus::Collision) => (MacTxResult::CollisionLimit, false),
            RetryOutcome::RetryLimit(_) => (MacTxResult::HardwareTimeout, false),
            RetryOutcome::Terminal(status) => (MacTxResult::HardwareFailure(status), false),
        };
        MacTxStatus {
            result,
            attempts: retry.attempts(),
            final_rate: exchange.rate,
            acknowledged: solicits_ack.then_some(delivered),
            ack_snr_db: if delivered { exchange.ack_snr_db } else { None },
            airtime_micros: None,
        }
    }

    /// Plan the attempt of the exchange's current phase.
    fn plan(
        &mut self,
        exchange: &mut TxExchange,
        live: Live,
        ladder: &impl RateLadder,
    ) -> TxAttemptPlan {
        let request = exchange.context;
        let (rate, psdu, content) = match live {
            Live::Mpdu(retry, mpdu) => (
                retry.rate(ladder, request.initial_rate, exchange.rate),
                TxPsdu::Mpdu {
                    length: mpdu.length,
                },
                AttemptContent::Mpdu {
                    set_retry_bit: exchange.transmitted != 0,
                },
            ),
            Live::Aggregate(state, ampdu) => {
                let subframes = state.pending_original_indices();
                exchange.aggregate = subframes;
                exchange.aggregate_attempts = exchange.aggregate_attempts.saturating_add(1);
                // A retained aggregate keeps its rate.
                (
                    request.initial_rate,
                    TxPsdu::Ampdu {
                        length: ampdu.aggregate_length(subframes),
                    },
                    AttemptContent::Ampdu {
                        subframes,
                        retry: exchange.transmitted & subframes,
                    },
                )
            }
            Live::BlockAckRequest(state, ampdu) => {
                exchange.aggregate = 0;
                let rate = PhyRate::Legacy(self.protection.control_rate(request.initial_rate));
                exchange.rate = rate;
                return TxAttemptPlan {
                    access_category: request.access_category,
                    rate,
                    protection: TxProtectionDecision::UNPROTECTED,
                    power: request.power,
                    coex: request.coex,
                    content: AttemptContent::BlockAckRequest {
                        tid: ampdu.tid,
                        starting_sequence: state.current_first_sequence(),
                    },
                };
            }
            Live::Individual {
                remaining,
                retry,
                length,
                ..
            } => {
                let index = remaining.trailing_zeros() as u8;
                let previous = if retry.attempts() == 1 {
                    request.initial_rate
                } else {
                    exchange.rate
                };
                (
                    retry.rate(ladder, request.initial_rate, previous),
                    TxPsdu::Mpdu { length },
                    AttemptContent::Subframe { index },
                )
            }
        };
        exchange.rate = rate;
        let protection = self.protection.select(
            ProtectedPpdu {
                rate,
                receiver: request.receiver,
                psdu,
            },
            &self.he_budget,
        );
        TxAttemptPlan {
            access_category: request.access_category,
            rate,
            protection,
            power: request.power,
            coex: request.coex,
            content,
        }
    }
}

impl TxAttemptPlan {
    /// The protection exchange of the attempt as the port requests it.
    pub const fn port_protection(&self) -> oer_ieee80211_lower_mac::Protection {
        self.protection.protection.port_protection()
    }

    /// Whether the attempt is protected by a control exchange.
    pub const fn is_protected(&self) -> bool {
        !matches!(self.protection.protection, TxProtection::None)
    }
}

/// The report's view of a BlockAck, for callers that keep their own
/// statistics: the subframes of `request` that `report` acknowledges.
pub fn acknowledged_subframes(request: &AmpduRequest, report: BlockAckReport) -> u64 {
    let mut acknowledged = 0_u64;
    for index in 0..request.subframes {
        if report.acknowledges(request.sequence(index)) {
            acknowledged |= 1 << index;
        }
    }
    acknowledged
}

#[cfg(test)]
mod tests;
