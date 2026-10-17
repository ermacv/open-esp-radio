//! Bounded, allocation-free FTM requester session policy.
//!
//! This owner implements the single-burst ASAP subset. It owns retry and
//! deadline state, peer/token identity and raw four-timestamp results, but it
//! performs no I/O. In particular, a chip backend must prove antenna-point
//! RX/TX timestamp capture before publishing the request returned by
//! [`FtmSession::service`].
//! One token set tracks all nonzero dialog tokens, including 255. Zero is
//! the protocol's absent-token value and cannot be inserted into that set.

use core::num::NonZeroU8;

use heapless::Vec;
use oer_ieee80211_mac::address::IndividualAddress;
use oer_ieee80211_mac::ftm::{
    FTM_INITIAL_REQUEST_BODY_LEN, FtmBurstDuration, FtmFormatAndBandwidth, FtmMeasurement,
    FtmRequestParameters, FtmResponseParameters, FtmResponseStatus, FtmTimestampPs, FtmToaError,
    FtmTodError, encode_initial_request,
};
use oer_time::{Duration, Instant};

const FTM_TIMESTAMP_MASK: u64 = (1_u64 << 48) - 1;
const FTM_TIMESTAMP_HALF_RANGE: u64 = 1_u64 << 47;
const MAX_REQUEST_ATTEMPTS: u8 = 8;
const MAX_TRACKED_INFORMATION_ELEMENTS_LEN: usize = 64;

/// A session's seen nonzero wire tokens, with one word-selection rule for
/// reads and writes. Eight words cover the entire u8 token space.
struct TokenSet([u32; 8]);

impl TokenSet {
    const EMPTY: Self = Self([0; 8]);

    fn contains(&self, token: u8) -> bool {
        NonZeroU8::new(token)
            .is_some_and(|token| *Self::word(token, self.0.each_ref()) & Self::bit(token) != 0)
    }

    fn insert(&mut self, token: NonZeroU8) {
        *Self::word(token, self.0.each_mut()) |= Self::bit(token);
    }

    fn word<T>(token: NonZeroU8, words: [T; 8]) -> T {
        let [a, b, c, d, e, f, g, h] = words;
        // Discarding five bits of a u8 leaves exactly the indices 0..=7.
        match token.get() >> 5 {
            0 => a,
            1 => b,
            2 => c,
            3 => d,
            4 => e,
            5 => f,
            6 => g,
            _ => h,
        }
    }

    const fn bit(token: NonZeroU8) -> u32 {
        1_u32 << (token.get() & 31)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmRequesterConfigError {
    ZeroResponseTimeout,
    ZeroRetryInterval,
    ZeroSessionTimeout,
    SessionShorterThanResponse,
    ZeroRequestAttempts,
    TooManyRequestAttempts,
    MultipleBurstsUnsupported,
    ScheduledBurstUnsupported,
    PartialTsfPreferenceUnsupported,
    NoMeasurementCountPreferenceUnsupported,
    UnsupportedRequestFormat,
}

/// One association-scoped request and retry policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmRequesterConfig {
    parameters: FtmRequestParameters,
    response_timeout: Duration,
    retry_interval: Duration,
    session_timeout: Duration,
    request_attempt_limit: u8,
}

impl FtmRequesterConfig {
    pub const fn new(
        parameters: FtmRequestParameters,
        response_timeout: Duration,
        retry_interval: Duration,
        session_timeout: Duration,
        request_attempt_limit: u8,
    ) -> Result<Self, FtmRequesterConfigError> {
        if response_timeout.as_micros() == 0 {
            return Err(FtmRequesterConfigError::ZeroResponseTimeout);
        }
        if retry_interval.as_micros() == 0 {
            return Err(FtmRequesterConfigError::ZeroRetryInterval);
        }
        if session_timeout.as_micros() == 0 {
            return Err(FtmRequesterConfigError::ZeroSessionTimeout);
        }
        if session_timeout.as_micros() < response_timeout.as_micros() {
            return Err(FtmRequesterConfigError::SessionShorterThanResponse);
        }
        if request_attempt_limit == 0 {
            return Err(FtmRequesterConfigError::ZeroRequestAttempts);
        }
        if request_attempt_limit > MAX_REQUEST_ATTEMPTS {
            return Err(FtmRequesterConfigError::TooManyRequestAttempts);
        }
        if parameters.number_of_bursts_exponent() != 0 {
            return Err(FtmRequesterConfigError::MultipleBurstsUnsupported);
        }
        if !parameters.asap() {
            return Err(FtmRequesterConfigError::ScheduledBurstUnsupported);
        }
        if parameters.partial_tsf_timer().is_some() {
            return Err(FtmRequesterConfigError::PartialTsfPreferenceUnsupported);
        }
        if parameters.ftms_per_burst() == 0 {
            return Err(FtmRequesterConfigError::NoMeasurementCountPreferenceUnsupported);
        }
        match parameters.format_and_bandwidth() {
            FtmFormatAndBandwidth::NoPreference
            | FtmFormatAndBandwidth::NonHt20Mhz
            | FtmFormatAndBandwidth::HtMixed20Mhz => {}
            _ => return Err(FtmRequesterConfigError::UnsupportedRequestFormat),
        }
        Ok(Self {
            parameters,
            response_timeout,
            retry_interval,
            session_timeout,
            request_attempt_limit,
        })
    }

    pub const fn parameters(self) -> FtmRequestParameters {
        self.parameters
    }

    /// Number of successfully transmitted FTM frames requested on the wire.
    pub const fn requested_ftms_per_burst(self) -> u8 {
        self.parameters.ftms_per_burst()
    }

    /// Maximum complete four-timestamp exchanges deliverable by that burst.
    ///
    /// The first FTM frame has no predecessor and the final frame supplies the
    /// responder timestamps for the preceding frame, so a single ASAP burst
    /// can deliver at most one fewer sample than its FTM frame allocation.
    pub const fn maximum_deliverable_samples(self) -> u8 {
        self.parameters.ftms_per_burst().saturating_sub(1)
    }

    pub const fn response_timeout(self) -> Duration {
        self.response_timeout
    }

    pub const fn retry_interval(self) -> Duration {
        self.retry_interval
    }

    pub const fn session_timeout(self) -> Duration {
        self.session_timeout
    }

    pub const fn request_attempt_limit(self) -> u8 {
        self.request_attempt_limit
    }
}

/// Timestamp capture for the just-received nonterminal FTM frame and its ACK.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmLocalExchangeTiming {
    /// A nonterminal exchange always names a nonzero dialog token.
    pub dialog_token: NonZeroU8,
    pub arrival_t2_ps: FtmTimestampPs,
    pub ack_departure_t3_ps: FtmTimestampPs,
}

impl FtmLocalExchangeTiming {
    pub const fn new(
        dialog_token: u8,
        arrival_t2_ps: FtmTimestampPs,
        ack_departure_t3_ps: FtmTimestampPs,
    ) -> Result<Self, FtmRequesterError> {
        let Some(dialog_token) = NonZeroU8::new(dialog_token) else {
            return Err(FtmRequesterError::ZeroLocalDialogToken);
        };
        Ok(Self {
            dialog_token,
            arrival_t2_ps,
            ack_departure_t3_ps,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmTimestampArithmeticError {
    ResponderClockDiscontinuous,
    AmbiguousResponderWrap,
    AmbiguousInitiatorWrap,
    NegativeRawInterval,
}

/// Raw interval difference from the four protocol timestamps.
///
/// This is not a calibrated RTT and cannot be converted to distance without a
/// chip-specific antenna delay, clock and timestamp-capture contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmRawIntervalDifferencePs {
    pub responder_round_trip_ps: u64,
    pub initiator_turnaround_ps: u64,
    pub difference_ps: u64,
}

/// One owned four-timestamp exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmRawExchange {
    pub dialog_token: u8,
    pub responder_departure_t1_ps: FtmTimestampPs,
    pub initiator_arrival_t2_ps: FtmTimestampPs,
    pub initiator_ack_departure_t3_ps: FtmTimestampPs,
    pub responder_ack_arrival_t4_ps: FtmTimestampPs,
    pub tod_error: FtmTodError,
    pub toa_error: FtmToaError,
}

impl FtmRawExchange {
    pub fn raw_interval_difference(
        self,
    ) -> Result<FtmRawIntervalDifferencePs, FtmTimestampArithmeticError> {
        if self.tod_error.not_continuous {
            return Err(FtmTimestampArithmeticError::ResponderClockDiscontinuous);
        }
        let responder_round_trip_ps = forward_timestamp_delta(
            self.responder_departure_t1_ps,
            self.responder_ack_arrival_t4_ps,
        )
        .ok_or(FtmTimestampArithmeticError::AmbiguousResponderWrap)?;
        let initiator_turnaround_ps = forward_timestamp_delta(
            self.initiator_arrival_t2_ps,
            self.initiator_ack_departure_t3_ps,
        )
        .ok_or(FtmTimestampArithmeticError::AmbiguousInitiatorWrap)?;
        let Some(difference_ps) = responder_round_trip_ps.checked_sub(initiator_turnaround_ps)
        else {
            return Err(FtmTimestampArithmeticError::NegativeRawInterval);
        };
        Ok(FtmRawIntervalDifferencePs {
            responder_round_trip_ps,
            initiator_turnaround_ps,
            difference_ps,
        })
    }
}

const fn forward_timestamp_delta(start: FtmTimestampPs, end: FtmTimestampPs) -> Option<u64> {
    let delta = end.get().wrapping_sub(start.get()) & FTM_TIMESTAMP_MASK;
    if delta < FTM_TIMESTAMP_HALF_RANGE {
        Some(delta)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmSessionTermination {
    PeerTerminalFrame,
}

/// Result returned by value so a later session cannot mutate old samples.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FtmSessionResult<const MAX_SAMPLES: usize> {
    pub peer: IndividualAddress,
    pub session_generation: u32,
    pub negotiated: FtmResponseParameters,
    pub termination: FtmSessionTermination,
    samples: Vec<FtmRawExchange, MAX_SAMPLES>,
}

impl<const MAX_SAMPLES: usize> FtmSessionResult<MAX_SAMPLES> {
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    pub fn sample(&self, index: usize) -> Option<FtmRawExchange> {
        self.samples.get(index).copied()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmSessionFailure {
    RequestTxFailed,
    InitialResponseTimedOut,
    SessionTimedOut,
    PeerIncapable,
    PeerFailed { retry_after_seconds: u8 },
    HardwareAdmissionRejected,
    ProtocolViolation,
}

/// Refusals of the requester state machine. Encoding checked initial
/// parameters into their fixed-size Action body is infallible.
///
/// ```compile_fail,E0599
/// use oer_ieee80211_sta::ftm::FtmRequesterError;
/// let impossible = FtmRequesterError::RequestEncode;
/// ```
///
/// ```
/// use oer_ieee80211_sta::ftm::FtmRequesterError;
/// let refused = FtmRequesterError::DeadlineOverflow;
/// assert!(matches!(refused, FtmRequesterError::DeadlineOverflow));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmRequesterError {
    CapacityTooSmall {
        required_samples: u8,
        capacity: usize,
    },
    GenerationExhausted,
    DeadlineOverflow,
    ZeroLocalDialogToken,
    MissingLocalTiming,
    UnexpectedLocalTiming,
    LocalDialogTokenMismatch,
    MissingInitialParameters,
    UnexpectedParameters,
    UnexpectedInformationElements,
    InformationElementsTooLong {
        length: usize,
        capacity: usize,
    },
    InitialRetransmissionMismatch,
    UnsupportedNegotiation,
    TooManyMeasurements,
    DialogTokenReused,
    DialogTokenOutOfSequence {
        expected: u8,
        actual: u8,
    },
    ConflictingDuplicate,
}

/// Diagnostic identity within one requester's sequence. Completion authority
/// is the owned transmission, never these copyable numbers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmOperationId {
    peer: IndividualAddress,
    session_generation: u32,
}

impl FtmOperationId {
    /// The individual peer validated before starting this request.
    pub const fn peer(self) -> IndividualAddress {
        self.peer
    }

    /// Diagnostic sequence within this requester, not completion authority.
    pub const fn session_generation(self) -> u32 {
        self.session_generation
    }
}

/// The input that releases a competing application's request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmWaitSource {
    /// The coordinator delivers this session's owned terminal, then may start
    /// the returned requester. No retry timer can substitute for that terminal.
    SessionTerminal(FtmOperationId),
}

/// A competing request remains with its caller until the named terminal.
#[must_use]
pub struct FtmOccupied<T> {
    pub request: T,
    pub wait: FtmWaitSource,
}

/// A scheduling/validation refusal before changing the retained owner.
#[must_use]
pub struct FtmRefusal<T> {
    pub error: FtmRequesterError,
    pub owner: T,
}

impl<T> core::fmt::Debug for FtmRefusal<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FtmRefusal")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// One transmission owns its complete session while the backend publishes it.
/// Completion and pre-publication rejection consume this handle; there is no
/// requester argument into which another session's terminal can be injected.
#[must_use]
pub struct FtmRequestTransmission<const MAX_SAMPLES: usize> {
    owner: SessionData<MAX_SAMPLES>,
    attempts_remaining: u8,
    attempt: u8,
    body: [u8; FTM_INITIAL_REQUEST_BODY_LEN],
}

impl<const N: usize> FtmRequestTransmission<N> {
    pub const fn session_generation(&self) -> u32 {
        self.owner.requester.session_generation
    }
    pub const fn transmission_generation(&self) -> u32 {
        self.owner.requester.transmission_generation
    }
    pub const fn attempt(&self) -> u8 {
        self.attempt
    }
    pub const fn body(&self) -> &[u8; FTM_INITIAL_REQUEST_BODY_LEN] {
        &self.body
    }

    /// Finish this exact transmission. A clock-range refusal returns the
    /// whole handle unchanged, so it can be cancelled or answered explicitly.
    #[allow(
        clippy::result_large_err,
        reason = "refusal returns the complete affine session without allocation"
    )]
    pub fn complete(
        self,
        acknowledged: bool,
        now: Instant,
    ) -> Result<(FtmSession<N>, FtmRequesterEvent), FtmRefusal<Self>> {
        let config = self.owner.requester.config;
        let (phase, event) = if acknowledged {
            let (Some(response_deadline), Some(session_deadline)) = (
                now.checked_add(config.response_timeout),
                now.checked_add(config.session_timeout),
            ) else {
                return Err(FtmRefusal {
                    error: FtmRequesterError::DeadlineOverflow,
                    owner: self,
                });
            };
            (
                FtmRequesterPhase::AwaitingInitial {
                    attempts_remaining: self.attempts_remaining,
                    response_deadline,
                    session_deadline,
                },
                FtmRequesterEvent::RequestPublished {
                    response_deadline,
                    session_deadline,
                },
            )
        } else if self.attempts_remaining == 0 {
            (
                FtmRequesterPhase::Failed(FtmSessionFailure::RequestTxFailed),
                FtmRequesterEvent::Failed(FtmSessionFailure::RequestTxFailed),
            )
        } else {
            let Some(retry_at) = now.checked_add(config.retry_interval) else {
                return Err(FtmRefusal {
                    error: FtmRequesterError::DeadlineOverflow,
                    owner: self,
                });
            };
            (
                FtmRequesterPhase::RequestQueued {
                    ready_at: retry_at,
                    attempts_remaining: self.attempts_remaining,
                },
                FtmRequesterEvent::RequestRetryScheduled { retry_at },
            )
        };
        Ok((
            FtmSession {
                owner: self.owner,
                phase,
            },
            event,
        ))
    }

    /// No DMA publication took place; return this session's owned failure.
    pub fn reject_hardware_admission(self) -> (FtmTerminal<N>, FtmRequesterEvent) {
        let reason = FtmSessionFailure::HardwareAdmissionRejected;
        (
            FtmSession {
                owner: self.owner,
                phase: FtmRequesterPhase::Failed(reason),
            }
            .cancel(),
            FtmRequesterEvent::Failed(reason),
        )
    }

    /// Cancel an unpublished transmission, retaining all collected samples.
    /// A backend that accepted publication must first supply its TX terminal.
    pub fn cancel(self) -> FtmTerminal<N> {
        let (requester, partial) = self.owner.into_parts();
        FtmTerminal {
            requester,
            outcome: FtmOutcome::Cancelled(partial),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmRequesterEvent {
    RequestPublished {
        response_deadline: Instant,
        session_deadline: Instant,
    },
    RequestRetryScheduled {
        retry_at: Instant,
    },
    Failed(FtmSessionFailure),
}

/// An owned step: either wait on the returned session's deadline/measurement,
/// perform its transmission, observe an event, or deliver its final result.
#[must_use]
pub enum FtmRequesterService<const N: usize> {
    Wait(FtmSession<N>),
    Transmit(FtmRequestTransmission<N>),
    Event {
        session: FtmSession<N>,
        event: FtmRequesterEvent,
    },
    Finished(FtmTerminal<N>),
}

/// Samples and the unmatched local timestamp pair survive failure/cancellation.
#[derive(Debug)]
pub struct FtmPartialResult<const N: usize> {
    pub operation: FtmOperationId,
    pub pending: Option<FtmLocalExchangeTiming>,
    samples: Vec<FtmRawExchange, N>,
}

impl<const N: usize> FtmPartialResult<N> {
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
    pub fn sample(&self, index: usize) -> Option<FtmRawExchange> {
        self.samples.get(index).copied()
    }
}

/// A terminal's complete outcome, including partial measurements on failure.
#[derive(Debug)]
pub enum FtmOutcome<const N: usize> {
    Complete(FtmSessionResult<N>),
    Failed {
        reason: FtmSessionFailure,
        partial: FtmPartialResult<N>,
    },
    Cancelled(FtmPartialResult<N>),
}

/// The only owner that returns the idle requester and its previous result.
#[must_use]
pub struct FtmTerminal<const N: usize> {
    requester: FtmRequester<N>,
    outcome: FtmOutcome<N>,
}

impl<const N: usize> FtmTerminal<N> {
    pub fn outcome(&self) -> &FtmOutcome<N> {
        &self.outcome
    }
    pub fn into_parts(self) -> (FtmRequester<N>, FtmOutcome<N>) {
        (self.requester, self.outcome)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmMeasurementDisposition {
    ForeignPeer,
    Stale,
    Duplicate,
    InitialAccepted {
        dialog_token: u8,
        allocated_ftms_per_burst: u8,
        maximum_samples: u8,
    },
    InitialRetransmissionAccepted {
        abandoned_dialog_token: u8,
        dialog_token: u8,
    },
    SampleAccepted {
        dialog_token: u8,
        sample_index: usize,
    },
    DuplicateSample {
        dialog_token: u8,
    },
    Complete {
        samples: usize,
    },
    Failed(FtmSessionFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FtmRequesterPhase {
    RequestQueued {
        ready_at: Instant,
        attempts_remaining: u8,
    },
    AwaitingInitial {
        attempts_remaining: u8,
        response_deadline: Instant,
        session_deadline: Instant,
    },
    Measuring {
        session_deadline: Instant,
        negotiated: FtmResponseParameters,
    },
    Complete {
        negotiated: FtmResponseParameters,
        termination: FtmSessionTermination,
    },
    Failed(FtmSessionFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LastMeasurement {
    fields: oer_ieee80211_mac::ftm::FtmMeasurementFields,
    parameters: Option<FtmResponseParameters>,
    information_elements: OwnedInformationElements,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OwnedInformationElements {
    bytes: [u8; MAX_TRACKED_INFORMATION_ELEMENTS_LEN],
    len: u8,
}

impl OwnedInformationElements {
    /// No element.
    const EMPTY: Self = Self {
        bytes: [0; MAX_TRACKED_INFORMATION_ELEMENTS_LEN],
        len: 0,
    };

    fn new(bytes: &[u8]) -> Result<Self, FtmRequesterError> {
        if bytes.len() > MAX_TRACKED_INFORMATION_ELEMENTS_LEN {
            return Err(FtmRequesterError::InformationElementsTooLong {
                length: bytes.len(),
                capacity: MAX_TRACKED_INFORMATION_ELEMENTS_LEN,
            });
        }
        let mut owned = [0_u8; MAX_TRACKED_INFORMATION_ELEMENTS_LEN];
        for (to, from) in owned.iter_mut().zip(bytes) {
            *to = *from;
        }
        Ok(Self {
            bytes: owned,
            len: bytes.len() as u8,
        })
    }

    fn matches(self, bytes: &[u8]) -> bool {
        usize::from(self.len) == bytes.len()
            && self
                .bytes
                .iter()
                .take(usize::from(self.len))
                .eq(bytes.iter())
    }
}

/// An idle requester. Starting consumes it; only the terminal of that session
/// returns it for another request. Numerical IDs are diagnostic, not authority.
pub struct FtmRequester<const MAX_SAMPLES: usize> {
    config: FtmRequesterConfig,
    session_generation: u32,
    transmission_generation: u32,
}

struct SessionData<const N: usize> {
    requester: FtmRequester<N>,
    peer: IndividualAddress,
    dialog_tokens: TokenSet,
    pending: Option<FtmLocalExchangeTiming>,
    last_measurement: Option<LastMeasurement>,
    samples: Vec<FtmRawExchange, N>,
}

impl<const N: usize> SessionData<N> {
    fn into_parts(self) -> (FtmRequester<N>, FtmPartialResult<N>) {
        let partial = FtmPartialResult {
            operation: FtmOperationId {
                peer: self.peer,
                session_generation: self.requester.session_generation,
            },
            pending: self.pending,
            samples: self.samples,
        };
        (self.requester, partial)
    }
}

/// One active operation, including any result waiting for the consumer.
#[must_use]
pub struct FtmSession<const N: usize> {
    owner: SessionData<N>,
    phase: FtmRequesterPhase,
}

impl<const N: usize> FtmRequester<N> {
    pub const fn new(config: FtmRequesterConfig) -> Self {
        Self {
            config,
            session_generation: 0,
            transmission_generation: 0,
        }
    }
    pub const fn config(&self) -> FtmRequesterConfig {
        self.config
    }

    #[allow(
        clippy::result_large_err,
        reason = "validation returns the original requester"
    )]
    pub fn start(
        mut self,
        peer: IndividualAddress,
        now: Instant,
    ) -> Result<FtmSession<N>, FtmRefusal<Self>> {
        let required_samples = self.config.maximum_deliverable_samples();
        if usize::from(required_samples) > N {
            return Err(FtmRefusal {
                error: FtmRequesterError::CapacityTooSmall {
                    required_samples,
                    capacity: N,
                },
                owner: self,
            });
        }
        if now.checked_add(self.config.session_timeout).is_none() {
            return Err(FtmRefusal {
                error: FtmRequesterError::DeadlineOverflow,
                owner: self,
            });
        }
        let Some(generation) = self.session_generation.checked_add(1) else {
            return Err(FtmRefusal {
                error: FtmRequesterError::GenerationExhausted,
                owner: self,
            });
        };
        self.session_generation = generation;
        let phase = FtmRequesterPhase::RequestQueued {
            ready_at: now,
            attempts_remaining: self.config.request_attempt_limit,
        };
        Ok(FtmSession {
            owner: SessionData {
                requester: self,
                peer,
                dialog_tokens: TokenSet::EMPTY,
                pending: None,
                last_measurement: None,
                samples: Vec::new(),
            },
            phase,
        })
    }
}

impl<const N: usize> FtmSession<N> {
    pub const fn operation(&self) -> FtmOperationId {
        FtmOperationId {
            peer: self.owner.peer,
            session_generation: self.owner.requester.session_generation,
        }
    }

    /// Retain an external request and name the terminal that admits its retry.
    pub fn refuse_request<T>(&self, request: T) -> FtmOccupied<T> {
        FtmOccupied {
            request,
            wait: FtmWaitSource::SessionTerminal(self.operation()),
        }
    }

    pub const fn next_deadline(&self) -> Option<Instant> {
        match self.phase {
            FtmRequesterPhase::RequestQueued { ready_at, .. } => Some(ready_at),
            FtmRequesterPhase::AwaitingInitial {
                response_deadline, ..
            } => Some(response_deadline),
            FtmRequesterPhase::Measuring {
                session_deadline, ..
            } => Some(session_deadline),
            FtmRequesterPhase::Complete { .. } | FtmRequesterPhase::Failed(_) => None,
        }
    }

    #[allow(
        clippy::result_large_err,
        reason = "scheduling refusal retains the complete session"
    )]
    pub fn service(mut self, now: Instant) -> Result<FtmRequesterService<N>, FtmRefusal<Self>> {
        match self.phase {
            FtmRequesterPhase::Complete { .. } | FtmRequesterPhase::Failed(_) => {
                return Ok(FtmRequesterService::Finished(self.cancel()));
            }
            FtmRequesterPhase::AwaitingInitial {
                attempts_remaining,
                response_deadline,
                ..
            } if now >= response_deadline => {
                if attempts_remaining == 0 {
                    self.phase =
                        FtmRequesterPhase::Failed(FtmSessionFailure::InitialResponseTimedOut);
                    return Ok(FtmRequesterService::Finished(self.cancel()));
                }
                let Some(retry_at) = now.checked_add(self.owner.requester.config.retry_interval)
                else {
                    return Err(FtmRefusal {
                        error: FtmRequesterError::DeadlineOverflow,
                        owner: self,
                    });
                };
                self.phase = FtmRequesterPhase::RequestQueued {
                    ready_at: retry_at,
                    attempts_remaining,
                };
                return Ok(FtmRequesterService::Event {
                    session: self,
                    event: FtmRequesterEvent::RequestRetryScheduled { retry_at },
                });
            }
            FtmRequesterPhase::Measuring {
                session_deadline, ..
            } if now >= session_deadline => {
                self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::SessionTimedOut);
                return Ok(FtmRequesterService::Finished(self.cancel()));
            }
            _ => {}
        }
        let FtmRequesterPhase::RequestQueued {
            ready_at,
            attempts_remaining,
        } = self.phase
        else {
            return Ok(FtmRequesterService::Wait(self));
        };
        if now < ready_at {
            return Ok(FtmRequesterService::Wait(self));
        }
        if now
            .checked_add(self.owner.requester.config.session_timeout)
            .is_none()
        {
            return Err(FtmRefusal {
                error: FtmRequesterError::DeadlineOverflow,
                owner: self,
            });
        }
        let Some(generation) = self.owner.requester.transmission_generation.checked_add(1) else {
            return Err(FtmRefusal {
                error: FtmRequesterError::GenerationExhausted,
                owner: self,
            });
        };
        self.owner.requester.transmission_generation = generation;
        let body = encode_initial_request(self.owner.requester.config.parameters);
        let attempt = self
            .owner
            .requester
            .config
            .request_attempt_limit
            .saturating_sub(attempts_remaining)
            .saturating_add(1);
        Ok(FtmRequesterService::Transmit(FtmRequestTransmission {
            owner: self.owner,
            attempts_remaining: attempts_remaining.saturating_sub(1),
            attempt,
            body,
        }))
    }

    /// Cancellation delivers a result already completed by RX unchanged.
    /// Otherwise it returns failure or partial samples and the unmatched local
    /// timing pair, together with the requester for a later session.
    pub fn cancel(self) -> FtmTerminal<N> {
        let (requester, partial) = self.owner.into_parts();
        let outcome = match self.phase {
            FtmRequesterPhase::Complete {
                negotiated,
                termination,
            } => FtmOutcome::Complete(FtmSessionResult {
                peer: partial.operation.peer(),
                session_generation: partial.operation.session_generation(),
                negotiated,
                termination,
                samples: partial.samples,
            }),
            FtmRequesterPhase::Failed(reason) => FtmOutcome::Failed { reason, partial },
            _ => FtmOutcome::Cancelled(partial),
        };
        FtmTerminal { requester, outcome }
    }

    pub fn on_measurement(
        &mut self,
        peer: IndividualAddress,
        measurement: FtmMeasurement<'_>,
        local_timing: Option<FtmLocalExchangeTiming>,
        now: Instant,
    ) -> Result<FtmMeasurementDisposition, FtmRequesterError> {
        if peer != self.owner.peer {
            return Ok(FtmMeasurementDisposition::ForeignPeer);
        }
        let session_deadline = match self.phase {
            FtmRequesterPhase::AwaitingInitial {
                session_deadline, ..
            }
            | FtmRequesterPhase::Measuring {
                session_deadline, ..
            } => session_deadline,
            FtmRequesterPhase::Complete { .. } | FtmRequesterPhase::Failed(_) => {
                return Ok(FtmMeasurementDisposition::Stale);
            }
            FtmRequesterPhase::RequestQueued { .. } => {
                return Ok(FtmMeasurementDisposition::Stale);
            }
        };
        if now >= session_deadline {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::SessionTimedOut);
            return Ok(FtmMeasurementDisposition::Failed(
                FtmSessionFailure::SessionTimedOut,
            ));
        }
        if let Some(last) = self.owner.last_measurement
            && last.fields.dialog_token == measurement.fields.dialog_token
        {
            if last.fields == measurement.fields
                && last.parameters == measurement.parameters
                && last
                    .information_elements
                    .matches(measurement.information_elements)
                && local_timing == self.owner.pending
            {
                return Ok(FtmMeasurementDisposition::Duplicate);
            }
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::ConflictingDuplicate);
        }

        if matches!(self.phase, FtmRequesterPhase::AwaitingInitial { .. }) {
            return self.accept_initial(measurement, local_timing, session_deadline);
        }
        match self.phase {
            FtmRequesterPhase::Measuring { negotiated, .. } => {
                self.accept_follow_up(measurement, local_timing, negotiated)
            }
            _ => Ok(FtmMeasurementDisposition::Stale),
        }
    }

    fn accept_initial(
        &mut self,
        measurement: FtmMeasurement<'_>,
        local_timing: Option<FtmLocalExchangeTiming>,
        session_deadline: Instant,
    ) -> Result<FtmMeasurementDisposition, FtmRequesterError> {
        if measurement.fields.follow_up_dialog_token != 0 {
            return Ok(FtmMeasurementDisposition::Stale);
        }
        let Some(negotiated) = measurement.parameters else {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::MissingInitialParameters);
        };
        match negotiated.status {
            FtmResponseStatus::Incapable => {
                self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::PeerIncapable);
                return Ok(FtmMeasurementDisposition::Failed(
                    FtmSessionFailure::PeerIncapable,
                ));
            }
            FtmResponseStatus::Failed {
                retry_after_seconds,
            } => {
                let failure = FtmSessionFailure::PeerFailed {
                    retry_after_seconds,
                };
                self.phase = FtmRequesterPhase::Failed(failure);
                return Ok(FtmMeasurementDisposition::Failed(failure));
            }
            FtmResponseStatus::Success => {}
        }
        self.validate_negotiation(negotiated)?;
        let information_elements =
            self.own_information_elements(measurement.information_elements)?;
        if measurement.fields.dialog_token == 0 {
            if local_timing.is_some() {
                return self.protocol_error(FtmRequesterError::UnexpectedLocalTiming);
            }
            self.owner.pending = None;
            self.owner.last_measurement = Some(LastMeasurement {
                fields: measurement.fields,
                parameters: measurement.parameters,
                information_elements,
            });
            self.phase = FtmRequesterPhase::Complete {
                negotiated,
                termination: FtmSessionTermination::PeerTerminalFrame,
            };
            return Ok(FtmMeasurementDisposition::Complete { samples: 0 });
        }
        if negotiated.ftms_per_burst == 1 {
            return self.protocol_error(FtmRequesterError::TooManyMeasurements);
        }
        let local_timing = self.validate_new_local_timing(measurement, local_timing)?;
        self.owner.dialog_tokens.insert(local_timing.dialog_token);
        self.owner.pending = Some(local_timing);
        self.owner.last_measurement = Some(LastMeasurement {
            fields: measurement.fields,
            parameters: measurement.parameters,
            information_elements,
        });
        self.phase = FtmRequesterPhase::Measuring {
            session_deadline,
            negotiated,
        };
        Ok(FtmMeasurementDisposition::InitialAccepted {
            dialog_token: measurement.fields.dialog_token,
            allocated_ftms_per_burst: negotiated.ftms_per_burst,
            maximum_samples: negotiated.ftms_per_burst.saturating_sub(1),
        })
    }

    fn accept_follow_up(
        &mut self,
        measurement: FtmMeasurement<'_>,
        local_timing: Option<FtmLocalExchangeTiming>,
        negotiated: FtmResponseParameters,
    ) -> Result<FtmMeasurementDisposition, FtmRequesterError> {
        if measurement.parameters.is_some() {
            return self.accept_initial_retransmission(measurement, local_timing, negotiated);
        }
        if !measurement.information_elements.is_empty() {
            return self.protocol_error(FtmRequesterError::UnexpectedInformationElements);
        }
        let Some(pending) = self.owner.pending else {
            return self.protocol_error(FtmRequesterError::MissingLocalTiming);
        };

        if measurement.fields.dialog_token == 0 && measurement.fields.follow_up_dialog_token == 0 {
            if local_timing.is_some() {
                return self.protocol_error(FtmRequesterError::UnexpectedLocalTiming);
            }
            if let Err(error) = self.complete_with_terminal(measurement, negotiated) {
                return self.protocol_error(error);
            }
            return Ok(FtmMeasurementDisposition::Complete {
                samples: self.owner.samples.len(),
            });
        }

        let sample = if measurement.fields.follow_up_dialog_token == pending.dialog_token.get() {
            Some(FtmRawExchange {
                dialog_token: pending.dialog_token.get(),
                responder_departure_t1_ps: measurement.fields.tod,
                initiator_arrival_t2_ps: pending.arrival_t2_ps,
                initiator_ack_departure_t3_ps: pending.ack_departure_t3_ps,
                responder_ack_arrival_t4_ps: measurement.fields.toa,
                tod_error: measurement.fields.tod_error,
                toa_error: measurement.fields.toa_error,
            })
        } else if self.is_last_body_retransmission(measurement) {
            None
        } else {
            return Ok(FtmMeasurementDisposition::Stale);
        };

        if measurement.fields.dialog_token == 0 {
            if local_timing.is_some() {
                return self.protocol_error(FtmRequesterError::UnexpectedLocalTiming);
            }
            let Some(sample) = sample else {
                return Ok(FtmMeasurementDisposition::Stale);
            };
            if self.owner.samples.len() >= usize::from(negotiated.ftms_per_burst.saturating_sub(1))
            {
                return self.protocol_error(FtmRequesterError::TooManyMeasurements);
            }
            self.push_sample(sample)?;
            if let Err(error) = self.complete_with_terminal(measurement, negotiated) {
                return self.protocol_error(error);
            }
            return Ok(FtmMeasurementDisposition::Complete {
                samples: self.owner.samples.len(),
            });
        }

        self.validate_next_dialog_token(
            pending.dialog_token.get(),
            measurement.fields.dialog_token,
        )?;
        let local_timing = self.validate_new_local_timing(measurement, local_timing)?;
        let maximum_samples = usize::from(negotiated.ftms_per_burst.saturating_sub(1));
        let disposition = if let Some(sample) = sample {
            if self.owner.samples.len().saturating_add(1) >= maximum_samples {
                return self.protocol_error(FtmRequesterError::TooManyMeasurements);
            }
            let sample_index = self.push_sample(sample)?;
            FtmMeasurementDisposition::SampleAccepted {
                dialog_token: pending.dialog_token.get(),
                sample_index,
            }
        } else {
            if self.owner.samples.len() >= maximum_samples {
                return self.protocol_error(FtmRequesterError::TooManyMeasurements);
            }
            FtmMeasurementDisposition::DuplicateSample {
                dialog_token: measurement.fields.follow_up_dialog_token,
            }
        };
        self.owner.dialog_tokens.insert(local_timing.dialog_token);
        self.owner.pending = Some(local_timing);
        self.owner.last_measurement = Some(LastMeasurement {
            fields: measurement.fields,
            parameters: None,
            information_elements: OwnedInformationElements::EMPTY,
        });
        Ok(disposition)
    }

    fn accept_initial_retransmission(
        &mut self,
        measurement: FtmMeasurement<'_>,
        local_timing: Option<FtmLocalExchangeTiming>,
        negotiated: FtmResponseParameters,
    ) -> Result<FtmMeasurementDisposition, FtmRequesterError> {
        let Some(pending) = self.owner.pending else {
            return self.protocol_error(FtmRequesterError::MissingLocalTiming);
        };
        let Some(last) = self.owner.last_measurement else {
            return self.protocol_error(FtmRequesterError::InitialRetransmissionMismatch);
        };
        if measurement.fields.dialog_token == 0
            || measurement.fields.follow_up_dialog_token != 0
            || measurement.parameters != Some(negotiated)
            || last.parameters != Some(negotiated)
            || last.fields.follow_up_dialog_token != 0
            || !last
                .information_elements
                .matches(measurement.information_elements)
        {
            return self.protocol_error(FtmRequesterError::InitialRetransmissionMismatch);
        }
        self.validate_next_dialog_token(
            pending.dialog_token.get(),
            measurement.fields.dialog_token,
        )?;
        let local_timing = self.validate_new_local_timing(measurement, local_timing)?;
        let information_elements =
            self.own_information_elements(measurement.information_elements)?;
        self.owner.dialog_tokens.insert(local_timing.dialog_token);
        self.owner.pending = Some(local_timing);
        self.owner.last_measurement = Some(LastMeasurement {
            fields: measurement.fields,
            parameters: measurement.parameters,
            information_elements,
        });
        Ok(FtmMeasurementDisposition::InitialRetransmissionAccepted {
            abandoned_dialog_token: pending.dialog_token.get(),
            dialog_token: measurement.fields.dialog_token,
        })
    }

    /// The session's end on its terminal frame.
    ///
    /// # Errors
    ///
    /// Information elements longer than the session keeps.
    fn complete_with_terminal(
        &mut self,
        measurement: FtmMeasurement<'_>,
        negotiated: FtmResponseParameters,
    ) -> Result<(), FtmRequesterError> {
        let information_elements = OwnedInformationElements::new(measurement.information_elements)?;
        self.owner.pending = None;
        self.owner.last_measurement = Some(LastMeasurement {
            fields: measurement.fields,
            parameters: measurement.parameters,
            information_elements,
        });
        self.phase = FtmRequesterPhase::Complete {
            negotiated,
            termination: FtmSessionTermination::PeerTerminalFrame,
        };
        Ok(())
    }

    fn validate_negotiation(
        &mut self,
        negotiated: FtmResponseParameters,
    ) -> Result<(), FtmRequesterError> {
        let requested = self.owner.requester.config.parameters;
        let request_format = requested.format_and_bandwidth();
        let format_allowed = matches!(
            negotiated.format_and_bandwidth,
            FtmFormatAndBandwidth::NonHt20Mhz | FtmFormatAndBandwidth::HtMixed20Mhz
        ) && (request_format == FtmFormatAndBandwidth::NoPreference
            || request_format == negotiated.format_and_bandwidth);
        if negotiated.number_of_bursts_exponent != 0
            || !negotiated.asap
            || !negotiated.asap_capable
            || negotiated.ftms_per_burst == 0
            || negotiated.ftms_per_burst > requested.ftms_per_burst()
            || usize::from(negotiated.ftms_per_burst.saturating_sub(1)) > N
            || negotiated.min_delta_ftm_100us < requested.min_delta_ftm_100us()
            || negotiated.burst_duration == FtmBurstDuration::NoPreference
            || negotiated.burst_duration.wire() > requested.burst_duration().wire()
            || negotiated.burst_period_100ms != 0
            || !format_allowed
        {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::UnsupportedNegotiation);
        }
        Ok(())
    }

    fn validate_next_dialog_token(
        &mut self,
        previous: u8,
        actual: u8,
    ) -> Result<(), FtmRequesterError> {
        if self.owner.dialog_tokens.contains(actual) {
            return self.protocol_error(FtmRequesterError::DialogTokenReused);
        }
        let expected = if previous == u8::MAX {
            1
        } else {
            previous.saturating_add(1)
        };
        if actual != expected {
            return self
                .protocol_error(FtmRequesterError::DialogTokenOutOfSequence { expected, actual });
        }
        Ok(())
    }

    fn own_information_elements(
        &mut self,
        bytes: &[u8],
    ) -> Result<OwnedInformationElements, FtmRequesterError> {
        match OwnedInformationElements::new(bytes) {
            Ok(elements) => Ok(elements),
            Err(error) => self.protocol_error(error),
        }
    }

    fn protocol_error<T>(&mut self, error: FtmRequesterError) -> Result<T, FtmRequesterError> {
        self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
        Err(error)
    }

    fn validate_new_local_timing(
        &mut self,
        measurement: FtmMeasurement<'_>,
        local_timing: Option<FtmLocalExchangeTiming>,
    ) -> Result<FtmLocalExchangeTiming, FtmRequesterError> {
        if measurement.fields.dialog_token == 0 {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::ZeroLocalDialogToken);
        }
        let Some(local_timing) = local_timing else {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::MissingLocalTiming);
        };
        if local_timing.dialog_token.get() != measurement.fields.dialog_token {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::LocalDialogTokenMismatch);
        }
        Ok(local_timing)
    }

    fn push_sample(&mut self, sample: FtmRawExchange) -> Result<usize, FtmRequesterError> {
        let index = self.owner.samples.len();
        if self.owner.samples.push(sample).is_err() {
            self.phase = FtmRequesterPhase::Failed(FtmSessionFailure::ProtocolViolation);
            return Err(FtmRequesterError::TooManyMeasurements);
        }
        Ok(index)
    }

    fn is_last_body_retransmission(&self, measurement: FtmMeasurement<'_>) -> bool {
        self.owner.last_measurement.is_some_and(|last| {
            last.parameters.is_none()
                && last.information_elements.matches(&[])
                && last.fields.follow_up_dialog_token == measurement.fields.follow_up_dialog_token
                && last.fields.tod == measurement.fields.tod
                && last.fields.toa == measurement.fields.toa
                && last.fields.tod_error == measurement.fields.tod_error
                && last.fields.toa_error == measurement.fields.toa_error
        })
    }
}

#[cfg(test)]
mod tests;
