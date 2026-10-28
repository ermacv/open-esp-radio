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

//! Espressif IEEE 802.11 transmit policy data.
//!
//! The portable algorithms above the lower-MAC port
//! (`oer-ieee80211-upper-mac`, `oer-ieee80211-wire`) take their limits, rate
//! ladders and estimates as parameters. This package holds the values the
//! Espressif stack uses, recovered from the pinned vendor libraries with
//! their provenance, for every chip of the family that runs that policy:
//!
//! - [`rate_schedule`]: the `libpp.a` rate-schedule arenas, their record
//!   walk and the rate-to-record maps;
//! - [`rate_control`]: the per-association rate control over those
//!   schedules, its selection and adaptation, as values;
//! - [`rate_code`]: the `wifi_phy_rate_t` codes of those records as portable
//!   rates;
//! - [`retry_ladder`]: the ordinary-MPDU retry ladder over the schedules,
//!   as a portable `RateLadder`;
//! - [`lmac`]: retry limits, A-MPDU lifetime and aging, the RTS threshold
//!   and the default EDCA contention;
//! - [`he_txop`]: the HE TXOP duration byte budget, as a portable
//!   `HeTxopRtsBudget`;
//! - [`ccmp`]: the transmit packet-number step;
//! - [`classification`]: the TX classifier's fixed-rate, priority and WMM
//!   admission decisions;
//! - [`connection_coex`]: the coexistence events and priorities of the
//!   station's connection frames under the reconnect policy;
//! - [`block_ack`]: the TIDs the station negotiates TX Block Ack for and
//!   its Dialog Token sequence.
//!
//! Where an algorithm is inseparable from the vendor bytes it walks (the
//! schedule walk and the retry ladder), it lives here beside them.

pub mod block_ack;
pub mod ccmp;
pub mod classification;
pub mod connection_coex;
pub mod he_txop;
pub mod lmac;
pub mod rate_code;
pub mod rate_control;
pub mod rate_record;
pub mod rate_schedule;
pub mod rate_selection;
pub mod retry_ladder;

pub use he_txop::EspressifHeTxopRtsBudget;
pub use retry_ladder::EspressifRetryLadder;

pub mod backend;
