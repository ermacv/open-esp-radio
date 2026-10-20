//! The values of a transmission and of its completion.
//!
//! An attempt of the phase ([`Attempt`](crate::Attempt)) is one frame
//! exchange by its retry plan ([`RetryPlan`](crate::RetryPlan)), which the
//! backend carries out below the port; its one [`TxOutcome`] reports how
//! it ended, how many transmissions it made and at which rate the last
//! went. The header the caller encodes lives in a transmit buffer of the
//! phase; the body, such as a network frame's payload, travels by
//! ownership as a [`TxBody`]. Whether the backend sends the body from its
//! owner's memory or copies it into its own is the backend's choice, not
//! the contract's.

use core::num::NonZeroU16;
use oer_ieee80211_wire::sequence::SequenceNumber;

/// The response the attempt solicits, which the hardware waits for.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxResponse {
    /// Nothing answers the frame: a group-addressed frame, or an
    /// individually addressed one sent without acknowledgement where the
    /// backend's
    /// [`individual_no_ack`](crate::LowerMacCapabilities::individual_no_ack)
    /// covers the rate.
    None,
    /// An individually addressed frame answered by an ACK.
    Ack,
    /// A BlockAckReq answered by a BlockAck, whose starting sequence and
    /// bitmap the completion reports.
    BlockAck,
}

/// The end of an MPDU that an attempt carries by ownership: a network
/// frame's payload, which the caller does not copy.
///
/// The backend holds the body from the attempt's admission until the
/// [`crate::LowerMacTerminal::TxCompleted`] carries it back. A `Sent` MPDU
/// retains its body inside the returned buffer for retransmission;
/// [`crate::TxBufferPool::renew`] returns both memory and body. A fresh
/// returned MPDU gives its body back beside its memory, and each subframe of
/// an aggregate holds its own. Dropping a body is how
/// its owner learns it is no longer needed: a consumer that drops the
/// completion event drops its bodies, and a backend that discards an
/// attempt, as an uninstall does, drops them with it.
pub trait TxBody {
    /// The octets the MPDU ends with.
    fn bytes(&self) -> &[u8];
}

/// No body: the body type of a backend part that takes attempts whose
/// bodies its owner already placed, such as a driver behind a backend that
/// copies them.
impl TxBody for core::convert::Infallible {
    fn bytes(&self) -> &[u8] {
        match *self {}
    }
}

/// Medium protection sent ahead of the PPDU in the same attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Protection {
    /// No RTS/CTS or CTS-to-self exchange before the PPDU.
    None,
    /// RTS/CTS exchange with the receiver.
    RtsCts,
    /// CTS addressed to the transmitter itself.
    CtsToSelf,
}

/// Transmit power of one attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxPower {
    /// The backend's calibrated target power for the rate.
    Calibrated,
    /// At most this many dBm: the backend sends the calibrated target when
    /// that is lower. Accepted down to
    /// [`tx_power_ceiling_min_dbm`](crate::LowerMacCapabilities::tx_power_ceiling_min_dbm).
    MaxDbm(i8),
}

/// One hardware publication observed by a retry planner. This is not a
/// terminal of a port attempt: the backend carries its plan through to
/// [`TxOutcome`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxStatus {
    /// The PPDU went out and the solicited response, if any, arrived.
    Success,
    /// No ACK or BlockAck arrived.
    AckTimeout,
    /// The protection exchange drew no CTS.
    CtsTimeout,
    /// The attempt lost contention or collided before a response.
    Collision,
    /// Hardware timed out without a completion; the publication outcome
    /// is unknown. This ends the attempt as Indeterminate, never a retry.
    TimedOut,
}

/// The BlockAck the recipient answered an A-MPDU or BlockAckReq with.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BlockAckReport {
    /// The sequence number represented by bit zero of `bitmap`.
    pub start_sequence: SequenceNumber,
    /// Bit `n` acknowledges `start_sequence + n`.
    pub bitmap: u64,
}

impl BlockAckReport {
    /// Positions of the 64-bit bitmap.
    pub const BITMAP_BITS: u16 = 64;

    /// Whether the transmitter may consider `sequence` delivered.
    ///
    /// Recipients use both standard-compliant starting-sequence conventions:
    /// some keep the oldest possible sequence and describe it with the
    /// bitmap, others advance the start to the first sequence not yet
    /// received. In the latter form an MPDU just before the window is
    /// already delivered although it has no bitmap bit. Only a bounded
    /// predecessor, at most one bitmap width behind the start, counts; a
    /// sequence beyond either side of the window stays unacknowledged, so a
    /// stale report cannot release newer traffic.
    pub const fn acknowledges(self, sequence: SequenceNumber) -> bool {
        let distance = self.start_sequence.forward_distance(sequence);
        if distance < Self::BITMAP_BITS {
            self.bitmap & (1_u64 << distance) != 0
        } else {
            sequence.forward_distance(self.start_sequence) <= Self::BITMAP_BITS
        }
    }
}

/// Why an admitted attempt was returned without publishing it again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WithdrawCause {
    /// The transmit gate closed.
    GateClosed,
    /// The attempt's key is being removed.
    KeyRemoved,
    /// The port is being disabled.
    Disabled,
}

/// Why a published MPDU's outcome cannot be established.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndeterminateCause {
    /// Hardware timed out and no completion arrived before detachment.
    HardwareTimeout,
}

/// The MPDU transmissions an attempt made, the first included, and the rate
/// of the last of them. It exists only when at least one went out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Air {
    /// Exact MPDU transmission count; this is not a publication count.
    pub transmissions: NonZeroU16,
    /// The rate the last transmission went at.
    pub rate: crate::PhyRate,
}

/// The SNR a backend's ACK evidence always carries: a property of the
/// backend, as [`SignalMetadata`](crate::SignalMetadata) is of its receive
/// metadata. A policy that needs it requires this trait of
/// [`PhaseTypes::AckEvidence`](crate::PhaseTypes::AckEvidence), and a
/// backend without it does not build with that policy.
pub trait AckSnr {
    /// The response's SNR, in dB.
    fn snr_db(&self) -> i8;
}

/// The SNR sample of a backend's BlockAck evidence for an aggregate, which
/// its hardware may not report for every accepted BlockAck: the evidence
/// type ([`PhaseTypes::BlockAckEvidence`](crate::PhaseTypes::BlockAckEvidence))
/// names why, and this trait splits it into a sample or none, with no
/// default value.
pub trait BlockAckSnr {
    /// The BlockAck's SNR sample.
    fn snr_sample(&self) -> SnrSample;
}

/// Whether a BlockAck carried an SNR measurement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnrSample {
    /// The backend measured the BlockAck's SNR, in dB.
    Measured(i8),
    /// The backend has no measurement of this BlockAck.
    Unmeasured,
}

/// The solicited response of a delivered single MPDU, with the backend's
/// evidence `A` of it. A frame that solicits no response has no evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleResponse<A> {
    /// The frame solicited no response: a group-addressed frame, or an
    /// individual one sent without acknowledgement.
    Unsolicited,
    /// An ACK answered the frame.
    Ack(A),
    /// A BlockAck answered a BlockAckReq.
    BlockAck {
        /// The BlockAck's sequence window.
        report: BlockAckReport,
        /// The backend's evidence of the response.
        ack: A,
    },
}

/// The known end of a single MPDU's plan, with the backend's response
/// evidence `A` and lost-air evidence `D`.
///
/// A delivery without a transmission is not representable. The response
/// variant is the one the backend observed; which response the frame
/// solicited is the frame's kind, which this outcome does not carry.
///
/// ```compile_fail,E0063
/// use oer_ieee80211_port::{SingleEnd, SingleResponse};
/// fn delivered() -> SingleEnd<(), ()> {
///     SingleEnd::Delivered { response: SingleResponse::Unsolicited }
/// }
/// ```
///
/// ```
/// use core::num::NonZeroU16;
/// use oer_ieee80211_port::{Air, PhyRate, SingleEnd, SingleResponse};
/// fn delivered(rate: PhyRate) -> SingleEnd<i8, ()> {
///     let air = Air { transmissions: NonZeroU16::MIN, rate };
///     SingleEnd::Delivered { air, response: SingleResponse::Ack(20) }
/// }
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleEnd<A, D> {
    /// The solicited response arrived, or the frame solicited none.
    Delivered {
        /// The transmissions it took.
        air: Air,
        /// The response that ended it.
        response: SingleResponse<A>,
    },
    /// The plan exhausted its ACK/BlockAck response budget.
    AckTimeout(Air),
    /// The plan exhausted its protection response budget; `None` when no
    /// MPDU of the attempt reached air.
    CtsTimeout(Option<Air>),
    /// Internal contention exhausted the backend's budget. No rate sample.
    ContentionLimit(Option<Air>),
    /// Coexistence denied the air, which the backend `D` tells apart. No
    /// rate sample.
    AirDenied {
        /// The backend's evidence of the denial.
        denial: D,
        /// The transmissions made before it.
        air: Option<Air>,
    },
}

/// The known end of an aggregate's plan, with the backend's BlockAck
/// evidence `B` and lost-air evidence `D`. A delivered aggregate always
/// carries its BlockAck: an aggregate solicits one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateEnd<B, D> {
    /// A BlockAck answered the aggregate.
    Delivered {
        /// The transmissions it took.
        air: Air,
        /// The BlockAck's sequence window.
        report: BlockAckReport,
        /// The backend's evidence of the BlockAck.
        ack: B,
    },
    /// The plan exhausted its BlockAck response budget.
    AckTimeout(Air),
    /// The plan exhausted its protection response budget; `None` when no
    /// MPDU of the attempt reached air.
    CtsTimeout(Option<Air>),
    /// Internal contention exhausted the backend's budget. No rate sample.
    ContentionLimit(Option<Air>),
    /// Coexistence denied the air. No rate sample.
    AirDenied {
        /// The backend's evidence of the denial.
        denial: D,
        /// The transmissions made before it.
        air: Option<Air>,
    },
}

/// One rate observation of an ended single MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateSample<'a, A> {
    /// An ACK or a BlockAck answered the frame.
    Acked {
        /// The transmissions it took.
        air: Air,
        /// The backend's evidence of the response.
        ack: &'a A,
    },
    /// The frame solicited no response and went out.
    Unsolicited(Air),
    /// The plan ended without its response.
    Unanswered(Air),
}

impl<A, D> SingleEnd<A, D> {
    /// The end's radio-rate observation: none for contention exhaustion,
    /// lost air or a protection failure before any MPDU went out.
    pub const fn rate_sample(&self) -> Option<RateSample<'_, A>> {
        match self {
            Self::Delivered {
                air,
                response: SingleResponse::Unsolicited,
            } => Some(RateSample::Unsolicited(*air)),
            Self::Delivered {
                air,
                response: SingleResponse::Ack(ack) | SingleResponse::BlockAck { ack, .. },
            } => Some(RateSample::Acked { air: *air, ack }),
            Self::AckTimeout(air) | Self::CtsTimeout(Some(air)) => {
                Some(RateSample::Unanswered(*air))
            }
            Self::CtsTimeout(None) | Self::ContentionLimit(_) | Self::AirDenied { .. } => None,
        }
    }
}

/// The sole terminal of an admitted operation, beside its owned resources;
/// `E` is a single MPDU's [`SingleEnd`] or an aggregate's [`AggregateEnd`].
///
/// A hardware timeout is final: the backend does not republish the MPDU.
/// A completion already observed takes priority over the timeout. Only
/// `Ended` claims an exact transmission count; rate selection ignores
/// withdrawals, indeterminate outcomes, contention exhaustion and lost air.
/// The next operation in the queue may proceed after every terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxOutcome<E> {
    /// Lifecycle stopped the attempt before its plan ended; no transmission
    /// delivered it. ReturnedMpdu is Fresh if never published, otherwise Sent,
    /// retaining the original SN, PN and body. No rate sample.
    Withdrawn(WithdrawCause),
    /// The plan has a known result. Returned MPDUs are `Sent`.
    Ended(E),
    /// Final unknown outcome. Returned MPDUs are `Sent`; Data retires them.
    Indeterminate(IndeterminateCause),
}

/// A single MPDU's terminal outcome.
pub type SingleOutcome<A, D> = TxOutcome<SingleEnd<A, D>>;
/// An aggregate's terminal outcome.
pub type AggregateOutcome<B, D> = TxOutcome<AggregateEnd<B, D>>;

impl<A, D> SingleOutcome<A, D> {
    /// The outcome's radio-rate observation, when it ended with one.
    pub const fn rate_sample(&self) -> Option<RateSample<'_, A>> {
        match self {
            Self::Ended(end) => end.rate_sample(),
            Self::Withdrawn(_) | Self::Indeterminate(_) => None,
        }
    }

    /// Whether the frame was delivered.
    pub const fn is_delivered(&self) -> bool {
        matches!(self, Self::Ended(SingleEnd::Delivered { .. }))
    }
}
