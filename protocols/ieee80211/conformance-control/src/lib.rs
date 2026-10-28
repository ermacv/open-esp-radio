#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods
    )
)]
//! Lower-MAC conformance on a dedicated diagnostic image.
mod payload;
pub use payload::*;
oer_hil_protocol::messages! {
    endpoint Prepare = "lower-mac/prepare" => Prepared;
    topic Prepared = "lower-mac/prepared";
    endpoint RunCase = "lower-mac/case/run" => CaseTrace;
    topic CaseTrace = "lower-mac/case/trace";
}

/// Reviewed paths, schemas (through keys) and message kinds.
#[cfg(feature = "registry")]
pub const MESSAGES_LOCK: &str = include_str!("../messages.lock");
