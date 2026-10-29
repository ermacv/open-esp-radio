#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::unreachable,
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods
    )
)]
//! Chip-independent HIL target logic.
//!
//! The HIL target runtimes compose these modules with their executor and
//! console; host tests exercise the same compiled code.
#![cfg_attr(not(test), no_std)]

pub mod base;
pub mod capture;
pub mod console;
pub mod lifecycle;
pub mod postmortem;

#[doc(hidden)]
pub use oer_hil_protocol as __protocol;
