//! What a retry policy is made of: its limits and the rates of its attempts.

use oer_protocols_ieee80211_wire::phy::PhyRate;

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

/// The rates of one frame's attempts: the peer's rate state.
pub trait RateLadder {
    /// The rate of the attempt that follows `failures` ladder-advancing
    /// failures of a frame first sent at `initial`; `failures` zero is the
    /// first attempt. `None` when the ladder has no rate for that step: the
    /// frame keeps its previous rate.
    fn rate(&self, initial: PhyRate, failures: u8) -> Option<PhyRate>;
}

impl<L: RateLadder + ?Sized> RateLadder for &L {
    fn rate(&self, initial: PhyRate, failures: u8) -> Option<PhyRate> {
        (**self).rate(initial, failures)
    }
}
