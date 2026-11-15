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
    clippy::disallowed_macros,
    clippy::disallowed_methods
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
        clippy::disallowed_methods,
    )
)]

//! Executor- and chip-independent IEEE 802.11 data plane.
//!
//! It holds today the station's data transmission over the port's two
//! data arenas ([`plane`]), the link's transmit numbers that plane and the
//! link's holder draw from ([`numbers`]), the MAC's transmit queues
//! ([`queues`]) and what the plane reports ([`events`]). The engine's
//! sections come with B3e and B3f.
//!
//! An MSDU comes in the network's memory, its Ethernet frame where the
//! plane reads it ([`MSDU_ETHERNET_OFFSET`]), and becomes one MPDU in
//! place, written once as a data arena of the port takes it.

#[cfg(test)]
extern crate std;

/// The length of an Ethernet header: destination, source and EtherType.
const ETHERNET_HEADER_LEN: usize = 14;

/// Where an MSDU's payload starts in a frame region: where the longest
/// data MPDU header ends, QoS Data with the CCMP header and LLC/SNAP. The
/// plane writes the header in front of the payload in place.
pub const MSDU_PAYLOAD_OFFSET: usize =
    oer_protocols_ieee80211_wire::data::IEEE80211_QOS_DATA_HEADER_LEN
        + oer_protocols_ieee80211_wire::ccmp::CCMP_HEADER_LEN
        + oer_protocols_ieee80211_wire::data::LLC_SNAP_HEADER_LEN;

/// Where a network places an MSDU's Ethernet frame in a frame region: its
/// header right before [`MSDU_PAYLOAD_OFFSET`].
pub const MSDU_ETHERNET_OFFSET: usize = MSDU_PAYLOAD_OFFSET - ETHERNET_HEADER_LEN;

/// The protection of a link's data MPDUs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataProtection<K> {
    /// An ordinary open MPDU, without a CCMP header or key.
    Open,
    /// A CCMP MPDU using the link's pairwise key.
    Ccmp(K),
}

/// Move the payload at `MSDU_PAYLOAD_OFFSET..end` down to start at `to`,
/// one shift-long run at a time: each run's source lies right behind its
/// destination, so the two never overlap.
fn move_down(output: &mut [u8], to: usize, end: usize) -> Option<()> {
    let shift = MSDU_PAYLOAD_OFFSET.checked_sub(to)?;
    output.get(..end)?;
    let mut at = to;
    while let Some(from) = at.checked_add(shift).filter(|&from| from < end) {
        let (front, back) = output.split_at_mut_checked(from)?;
        let run = shift.min(end.checked_sub(from)?);
        let destination = front.get_mut(at..)?.get_mut(..run)?;
        for (octet, source) in destination.iter_mut().zip(back.get(..run)?) {
            *octet = *source;
        }
        at = from;
    }
    Some(())
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
pub mod events;
pub mod numbers;
pub mod plane;
pub mod queues;

#[cfg(test)]
mod tests;
