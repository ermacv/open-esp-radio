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
    clippy::disallowed_macros
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
    )
)]

//! Executor- and chip-independent Wi-Fi station MLME and policy.
//!
//! Protocol crates own scan records, IEEE 802.11 framing and WPA state. This
//! crate owns the station: [`machine`], one sans-IO machine for the whole
//! lifecycle, its Authentication/Association state machines ([`join`]), its
//! power-save decisions and the application's station vocabulary
//! ([`station`]). It never waits: a thin service loop over the port, the
//! channel arbiter and time drives it.
//!
//! The station owns its SAE exchange and PMKSA cache. Personal credentials
//! carry both an SAE password and a prederived PSK PMK; network selection
//! prefers a qualified SAE offer, including WPA2/WPA3 transition networks.

#[cfg(test)]
extern crate std;

mod counter;
pub use counter::Counter;

pub mod ftm;
pub mod join;
pub mod link_monitor;
pub mod machine;
pub mod modem_sleep;
mod pmksa;
mod pool;
pub mod power_save;
pub mod request;
pub mod sa_query;
pub mod station;
pub mod time;
pub mod twt;

pub mod association;
pub mod data;
pub mod epoch;
