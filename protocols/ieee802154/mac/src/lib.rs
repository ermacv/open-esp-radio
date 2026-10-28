#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::arithmetic_side_effects,
        clippy::unreachable
    )
)]

//! Sans-IO IEEE 802.15.4 MAC core: retry policy, key/counter ownership,
//! CSL phase and network-time calculation. Backends use the separate
//! `oer-ieee802154-port` and `oer-ieee802154-wire` crates.

#[cfg(test)]
extern crate std;

/// MAC policy and state machines.
pub mod mac {
    /// CSL sample phase computation.
    pub mod csl;
    /// Frame retries after a failed attempt.
    pub mod retransmission;
    /// MAC key selection and counter allocation.
    pub mod security;
    /// Network-time computation and Time IE preparation.
    pub mod time_sync;
}

pub use mac::csl::{CSL_UNIT_MICROS, csl_phase};
pub use mac::retransmission::{AttemptFailure, FrameRetries, RetryDelayError, RetryStart};
pub use mac::security::MacKeys;
pub use mac::time_sync::write_time_sync;
