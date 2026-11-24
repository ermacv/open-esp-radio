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

//! Portable IEEE 802.11 frame-exchange state above the lower-MAC port.
//!
//! One lower-MAC submission is one hardware transmission attempt
//! (`oer-protocols-ieee80211-contracts-port`). Everything a backend does not do on its own
//! lies above that port and is decided here, once for every backend:
//!
//! - [`retry`]: the limits of one MPDU's retries and the [`RateLadder`] its
//!   attempts walk;
//! - [`ampdu`]: the limits of an A-MPDU's retries;
//! - [`protection`]: RTS/CTS or CTS-to-self before each PPDU;
//! - [`management`]: the transmission budget of a management frame;
//! - [`phy_mode`]: the PHY mode of one association and the elements its
//!   request carries;
//! - [`rate_control`]: the seam of a peer's data rates, which learns from
//!   its exchanges.
//!
//! Receive owners retain duplicate history, fragment reassembly, CCMP replay
//! and Block Ack reordering. [`monitor`] selects the frames a monitor keeps and
//! the channels it visits; [`vif`] routes a received MPDU of a same-channel
//! station and SoftAP pair to its interface. Transmit owners retain PN allocation,
//! the [`sequence`] number spaces and Block Ack negotiation/originator windows. Wire formats remain in
//! `oer-protocols-ieee80211-wire`.
//!
//! The package is sans-IO: completions and the radio time enter as values
//! and decisions leave as values, and nothing waits. The station
//! service in `oer-protocols-ieee80211-runtime` submits to the port and feeds its
//! terminals into rate selection ([`rate_selection::RateSelection::learn`]). Vendor retry schedules, limits and
//! duration estimates are parameters a profile supplies, such as
//! `oer-chips-family-radio-ieee80211-policy`.

#[cfg(test)]
extern crate std;

pub mod ampdu;
pub mod management;
pub mod phy_mode;
pub mod protection;
pub mod qos;
pub mod rate_control;
pub mod rate_selection;
pub mod retry;
pub mod sequence;

pub use ampdu::AmpduRetryPolicy;
pub use protection::{HeTxopRtsBudget, ProtectEveryHeTxop, ProtectionPolicy, RtsThreshold};
pub use retry::{AckFailureAccounting, RateLadder, RetryLimits};

pub mod block_ack;

pub mod ccmp;
pub mod packet_number;

pub mod duplicate;

pub mod fragmentation;

pub mod receive;

pub mod monitor;

pub mod vif;

/// A literal test sequence number; panics outside twelve bits.
#[cfg(test)]
pub(crate) const fn seq(value: u16) -> oer_protocols_ieee80211_wire::sequence::SequenceNumber {
    match oer_protocols_ieee80211_wire::sequence::SequenceNumber::new(value) {
        Some(sequence) => sequence,
        None => panic!("test sequence number exceeds twelve bits"),
    }
}
