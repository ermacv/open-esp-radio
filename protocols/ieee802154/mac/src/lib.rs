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

//! Sans-IO IEEE 802.15.4 MAC core: unslotted CSMA-CA backoff over the
//! backend's single clear-channel assessment, retry policy, key/counter
//! ownership, CSL phase and network-time calculation. Backends use the
//! separate `oer-ieee802154-port` and `oer-ieee802154-wire` crates.

#[cfg(test)]
extern crate std;

/// CSL sample phase computation.
pub mod csl;
/// Unslotted CSMA-CA backoff between single clear-channel assessments.
pub mod csma;
/// Frame retries after a failed attempt.
pub mod retransmission;
/// MAC key selection and counter allocation.
pub mod security;
/// Network-time computation and Time IE preparation.
pub mod time_sync;

pub use csl::{CSL_UNIT_MICROS, csl_phase};
pub use csma::CsmaCa;
pub use retransmission::{AttemptFailure, FrameRetries, RetryDelayError, RetryStart};
pub use security::MacKeys;
pub use time_sync::write_time_sync;
