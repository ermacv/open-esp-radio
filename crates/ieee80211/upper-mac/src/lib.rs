#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
#![cfg_attr(
    test,
    allow(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
        clippy::disallowed_macros,
    )
)]

//! Portable IEEE 802.11 transmit policy above the lower-MAC port.
//!
//! One lower-MAC submission is one hardware transmission attempt
//! (`oer-ieee80211-lower-mac`). Everything a backend does not do on its own
//! lies above that port and is decided here, once for every backend:
//!
//! - [`retry`]: the retry counters, limits and Retry bit of one MPDU, and the
//!   [`RateLadder`] its attempts walk;
//! - [`aggregate`]: how many queued frames one A-MPDU carries;
//! - [`ampdu`]: which subframes of an A-MPDU the next attempt resends after
//!   a BlockAck, with their Retry bit, and when the aggregate ends;
//! - [`protection`]: RTS/CTS or CTS-to-self before each PPDU;
//! - [`rate_control`]: the seam of a peer's data rates, which learns from
//!   its exchanges;
//! - [`tx`]: the [`TxPlanner`], which combines them into one attempt at a
//!   time and turns each completion into the next attempt or the exchange's
//!   statistics. It draws no backoff: the lower-MAC port carries none, and
//!   a backend draws its queues' contention below the port.
//!
//! The package is sans-IO: completions and the radio time enter as values,
//! attempt plans and reports leave as values, and nothing waits. The station
//! service in `oer-ieee80211-runtime` submits to the port and feeds its
//! terminals into rate selection. Vendor retry schedules, limits and
//! duration estimates are parameters a profile supplies, such as
//! `oer-espressif-ieee80211-policy`.

pub mod aggregate;
pub mod ampdu;
pub mod protection;
pub mod rate_control;
pub mod rate_selection;
pub mod retry;
pub mod tx;

pub use ampdu::{
    AmpduAttemptResult, AmpduRetryDecision, AmpduRetryError, AmpduRetryPolicy, AmpduRetryState,
};
pub use protection::{
    BasicRates, BssProtection, HePacketPadding, HeTxopDurationRtsThreshold, HeTxopRtsBudget,
    ProtectEveryHeTxop, ProtectedPpdu, ProtectionPolicy, RtsLengthThreshold, TxProtection,
    TxProtectionDecision, TxProtectionReasons, TxPsdu, TxReceiver,
};
pub use retry::{
    AckFailureAccounting, FixedRate, FrameClass, MpduRetryState, RateLadder, RetryCounters,
    RetryDecision, RetryLimits, RetryOutcome,
};
pub use tx::{
    AmpduRequest, AttemptContent, MpduRequest, TxAttemptPlan, TxBody, TxExchange, TxPlanError,
    TxPlanner, TxReport, TxRequest, TxStep,
};
