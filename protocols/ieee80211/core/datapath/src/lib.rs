#![expect(
    clippy::cast_possible_truncation,
    reason = "R3 cleans the cores and the contracts (tasks/312)"
)]
#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::let_underscore_must_use, clippy::let_underscore_untyped)]
#![cfg_attr(
    test,
    allow(clippy::let_underscore_must_use, clippy::let_underscore_untyped)
)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

//! Executor- and chip-independent IEEE 802.11 data plane.
//!
//! It holds today the station's data transmission over the port's two
//! data arenas ([`plane`]), the link's transmit numbers that plane and the
//! link's holder draw from ([`numbers`]), the MAC's transmit queues
//! ([`queues`]) and what the plane reports ([`events`]).
//!
//! An MSDU comes in the network's memory, its Ethernet header at the end
//! of the headroom in front of its payload ([`MSDU_ETHERNET_OFFSET`]), and
//! becomes one MPDU in place: the plane writes its front into the headroom,
//! flush against the payload, once, as a data arena of the port takes it.
//! What one attempt costs on the air is the plane's arithmetic ([`cost`]).

#[cfg(test)]
extern crate std;

/// The length of an Ethernet header: destination, source and EtherType.
const ETHERNET_HEADER_LEN: usize = 14;

/// Where an MSDU's payload starts in the network's memory: after the
/// port's headroom, in front of which the plane writes the MPDU's front.
pub const MSDU_PAYLOAD_OFFSET: usize = oer_protocols_ieee80211_contracts_port::HEADROOM;

/// Where a network places an MSDU's Ethernet frame in its memory: its
/// header the headroom's last octets, right before [`MSDU_PAYLOAD_OFFSET`].
pub const MSDU_ETHERNET_OFFSET: usize = MSDU_PAYLOAD_OFFSET - ETHERNET_HEADER_LEN;

/// The protection of a link's data MPDUs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataProtection<K> {
    /// An ordinary open MPDU, without a CCMP header or key.
    Open,
    /// A CCMP MPDU using the link's pairwise key.
    Ccmp(K),
}

/// The calibrated power every attempt takes, which its port must offer:
/// checked when the caller's type is built, so a port without it builds no
/// caller.
pub const fn calibrated_power<K: oer_protocols_ieee80211_contracts_port::Ieee80211Capabilities>()
-> oer_protocols_ieee80211_contracts_port::SupportedPower<K> {
    const {
        match oer_protocols_ieee80211_contracts_port::SupportedPower::new(
            oer_protocols_ieee80211_contracts_port::TxPower::Calibrated,
        ) {
            Some(power) => power,
            None => panic!("a port sends an attempt at its calibrated power"),
        }
    }
}

mod aggregate;
pub use aggregate::{AggregateRetry, MAX_AGGREGATE, PpduLimit};
pub mod cost;
pub mod engine;
pub mod events;
pub mod numbers;
pub mod plane;
pub mod queues;

#[cfg(test)]
mod model;
