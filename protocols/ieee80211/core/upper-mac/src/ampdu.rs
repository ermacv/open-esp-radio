//! The limits of an A-MPDU's retries.

use oer_portable_time::Duration;

/// The limits of one aggregate's retries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduRetryPolicy {
    /// Time from an MSDU's admission after which its subframe is aged and
    /// discarded instead of retried.
    pub lifetime: Duration,
    /// An MSDU with less than this much lifetime left is already aged.
    pub aged_margin: Duration,
    /// Attempts that end without any BlockAck, and failed protection
    /// exchanges in a row, that the aggregate survives.
    pub retry_limit: u8,
    /// Keep one missing MPDU in an aggregate instead of sending it alone.
    pub retain_single_mpdu: bool,
}
