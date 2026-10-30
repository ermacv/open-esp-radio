#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
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
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::indexing_slicing,
        clippy::unwrap_used
    )
)]

//! Ethernet, VLAN, IP and transport header fields.
//!
//! The codec owns byte order, bit fields, lengths and the structural
//! dependencies between these fields. It returns checked values and borrowed
//! views of the caller's bytes and writes into the caller's buffer. A parse
//! tells a malformed header from a valid one it does not interpret; it never
//! decides what a packet means for a radio.

pub mod ethernet;
pub mod ip;
pub mod transport;
pub mod vlan;
