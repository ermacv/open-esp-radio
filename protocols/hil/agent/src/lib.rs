#![expect(
    clippy::cast_possible_truncation,
    reason = "R3 cleans the cores and the contracts (tasks/312)"
)]
#![expect(
    clippy::let_underscore_must_use,
    reason = "R3 cleans the cores and the contracts (tasks/312)"
)]
#![forbid(unsafe_code)]
//! Chip-independent HIL target logic.
//!
//! The HIL target runtimes compose these modules with their executor and
//! console; host tests exercise the same compiled code.
#![cfg_attr(not(test), no_std)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

pub mod base;
pub mod capture;
pub mod console;
pub mod lifecycle;
pub mod postmortem;

#[cfg(test)]
mod test_messages;

#[doc(hidden)]
pub use oer_protocols_hil_protocol as __protocol;
