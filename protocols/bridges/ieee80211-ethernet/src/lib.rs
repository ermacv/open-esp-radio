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

//! The bridge between Ethernet II frames and IEEE 802.11 MSDUs.
//!
//! It classifies an outgoing frame's requested user priority from its VLAN
//! and IP fields. Each format stays its codec's: Ethernet, VLAN and IP fields
//! are `oer-network-wire`'s, the priority values `oer-ieee80211-wire`'s. An
//! IEEE 802.11 protocol crate reaches network fields only through this
//! crate.

#[cfg(test)]
extern crate std;

pub mod qos;
