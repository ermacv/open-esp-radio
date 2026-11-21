#![cfg_attr(
    test,
    expect(
        clippy::cast_possible_truncation,
        reason = "test code: a cast or a dropped result in a test loses no product's value"
    )
)]
#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

//! The bridge between Ethernet II frames and IEEE 802.11 MSDUs.
//!
//! It classifies an outgoing frame's requested user priority from its VLAN
//! and IP fields. Each format stays its codec's: Ethernet, VLAN and IP fields
//! are `oer-protocols-network-wire`'s, the priority values `oer-protocols-ieee80211-wire`'s. An
//! IEEE 802.11 protocol crate reaches network fields only through this
//! crate.

#[cfg(test)]
extern crate std;

pub mod qos;
