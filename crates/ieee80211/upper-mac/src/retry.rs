//! Retry policy of one individually addressed MPDU.
//!
//! One hardware publication supplies one [`TxStatus`]. [`MpduRetryState`]
//! decides from it whether the frame is done or is sent again and whether the
//! next attempt sets the Retry bit; a backend that draws its own contention
//! resets its window when the frame is done and doubles it when it is sent
//! again. It keeps the retry counters of the EDCA retransmit
//! procedures (IEEE Std 802.11-2020 10.23.2): a short counter for failures of short frames and of the
//! RTS/CTS exchange, a long counter for failures of long frames, and a
//! counter of the frame's own unacknowledged transmissions. The limits and
//! the way an ACK timeout is counted are parameters ([`RetryLimits`]),
//! because implementations differ in both; a vendor profile supplies its
//! own values.
//!
//! The rate of each attempt comes from a [`RateLadder`], the peer's rate
//! state: the attempt after `n` ladder-advancing failures uses the ladder's
//! rate for `n`. An unknown hardware timeout ends the attempt immediately:
//! `TimedOut` leaves retry counters unchanged and never republishes the MPDU.
//! Backend faults are carried by Poisoned and do not enter this policy.

use oer_ieee80211_lower_mac::TxStatus;
use oer_ieee80211_mac::phy::PhyRate;

/// Which retry counter a collision of the frame advances: IEEE 802.11
/// classifies a frame as long when it is longer than dot11RTSThreshold.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FrameClass {
    #[default]
    Short,
    Long,
}

/// How an ACK timeout is counted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AckFailureAccounting {
    /// The frame's class counter: a long frame advances the long counter,
    /// as IEEE 802.11 counts it.
    ByFrameClass,
    /// Always the short counter, whatever the frame's length. An
    /// implementation that reaches its long-retry path only for frames sent
    /// inside a granted TXOP counts this way when it holds no TXOP.
    Short,
}

/// The limits of one frame's retries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryLimits {
    /// dot11ShortRetryLimit: failed attempts the short counter admits.
    pub short: u8,
    /// dot11LongRetryLimit: failed attempts the long counter admits.
    pub long: u8,
    pub ack_failure: AckFailureAccounting,
}

impl RetryLimits {
    /// The IEEE 802.11 defaults: seven short and four long retries.
    pub const IEEE_DEFAULT: Self = Self {
        short: 7,
        long: 4,
        ack_failure: AckFailureAccounting::ByFrameClass,
    };
}

/// The counters of one frame's failed attempts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RetryCounters {
    /// Unacknowledged transmissions of the frame itself.
    pub mpdu: u8,
    pub short: u8,
    pub long: u8,
}

impl RetryCounters {
    /// Failures that walk the rate ladder: every ACK timeout and every
    /// failure the short counter records. A long-frame collision leaves the
    /// rate where it is.
    pub const fn ladder_step(self) -> u8 {
        if self.mpdu > self.short {
            self.mpdu
        } else {
            self.short
        }
    }
}

/// The rates of one frame's attempts: the peer's rate state.
pub trait RateLadder {
    /// The rate of the attempt that follows `failures` ladder-advancing
    /// failures of a frame first sent at `initial`; `failures` zero is the
    /// first attempt. `None` when the ladder has no rate for that step: the
    /// frame keeps its previous rate.
    fn rate(&self, initial: PhyRate, failures: u8) -> Option<PhyRate>;
}

/// Every attempt at the initial rate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FixedRate;

impl RateLadder for FixedRate {
    fn rate(&self, initial: PhyRate, _failures: u8) -> Option<PhyRate> {
        Some(initial)
    }
}

impl<L: RateLadder + ?Sized> RateLadder for &L {
    fn rate(&self, initial: PhyRate, failures: u8) -> Option<PhyRate> {
        (**self).rate(initial, failures)
    }
}

/// How the frame ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryOutcome {
    /// The solicited response arrived.
    Delivered,
    /// A retry limit was reached; the attempt that reached it ended with
    /// this status.
    RetryLimit(TxStatus),
    /// The attempt ended in a way no retry repairs (`Aborted` or `Fault`).
    Terminal(TxStatus),
}

/// What the next step of the frame is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryDecision {
    Complete(RetryOutcome),
    /// Send the same encoded MPDU again. Only an ACK timeout sets the IEEE
    /// 802.11 Retry bit: after a CTS timeout or a collision the MPDU never
    /// reached the receiver.
    Retry {
        set_retry_bit: bool,
    },
}

/// Why a retry state was not created.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryStateError {
    /// A frame is sent at least once.
    ZeroMpduRetryLimit,
}

/// The retry state of one MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MpduRetryState {
    limits: RetryLimits,
    mpdu_retry_limit: u8,
    frame_class: FrameClass,
    counters: RetryCounters,
    attempts: u8,
}

impl MpduRetryState {
    /// The state of a frame about to be sent the first time; at most
    /// `mpdu_retry_limit` of its transmissions go unacknowledged.
    pub const fn new(
        limits: RetryLimits,
        mpdu_retry_limit: u8,
        frame_class: FrameClass,
    ) -> Result<Self, RetryStateError> {
        if mpdu_retry_limit == 0 {
            return Err(RetryStateError::ZeroMpduRetryLimit);
        }
        Ok(Self {
            limits,
            mpdu_retry_limit,
            frame_class,
            counters: RetryCounters {
                mpdu: 0,
                short: 0,
                long: 0,
            },
            attempts: 1,
        })
    }

    pub const fn counters(&self) -> RetryCounters {
        self.counters
    }

    /// Attempts made, the first included.
    pub const fn attempts(&self) -> u8 {
        self.attempts
    }

    /// The rate of the current attempt; `previous` when the ladder has no
    /// rate for the current step.
    pub fn rate(&self, ladder: &impl RateLadder, initial: PhyRate, previous: PhyRate) -> PhyRate {
        ladder
            .rate(initial, self.counters.ladder_step())
            .unwrap_or(previous)
    }

    /// Apply the status of the attempt just completed.
    pub fn observe(&mut self, status: TxStatus) -> RetryDecision {
        match status {
            TxStatus::Success => Self::end(RetryOutcome::Delivered),
            TxStatus::AckTimeout => {
                self.counters.mpdu = self.counters.mpdu.saturating_add(1);
                let class_reached = match (self.limits.ack_failure, self.frame_class) {
                    (AckFailureAccounting::Short, _)
                    | (AckFailureAccounting::ByFrameClass, FrameClass::Short) => self.count_short(),
                    (AckFailureAccounting::ByFrameClass, FrameClass::Long) => self.count_long(),
                };
                let reached = class_reached || self.counters.mpdu >= self.mpdu_retry_limit;
                self.finish_or_retry(status, reached, true)
            }
            TxStatus::CtsTimeout => {
                let reached = self.count_short();
                self.finish_or_retry(status, reached, false)
            }
            TxStatus::Collision => {
                let reached = match self.frame_class {
                    FrameClass::Short => self.count_short(),
                    FrameClass::Long => self.count_long(),
                };
                self.finish_or_retry(status, reached, false)
            }
            TxStatus::TimedOut => Self::end(RetryOutcome::Terminal(status)),
        }
    }

    fn count_short(&mut self) -> bool {
        self.counters.short = self.counters.short.saturating_add(1);
        self.counters.short >= self.limits.short
    }

    fn count_long(&mut self) -> bool {
        self.counters.long = self.counters.long.saturating_add(1);
        self.counters.long >= self.limits.long
    }

    const fn end(outcome: RetryOutcome) -> RetryDecision {
        RetryDecision::Complete(outcome)
    }

    fn finish_or_retry(
        &mut self,
        status: TxStatus,
        reached: bool,
        set_retry_bit: bool,
    ) -> RetryDecision {
        if reached {
            Self::end(RetryOutcome::RetryLimit(status))
        } else {
            self.attempts = self.attempts.saturating_add(1);
            RetryDecision::Retry { set_retry_bit }
        }
    }
}

#[cfg(test)]
mod tests;
