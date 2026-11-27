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

//! The coexistence event priority table every supported Espressif chip
//! shares, recovered from esp-coex-lib: the events ([`CoexEventId`]), the
//! priority domain ([`CoexPti`]) and the vendor's table ([`CoexPtiTable`]).
//! It owns no register and keeps no state.

#[cfg(test)]
extern crate std;

mod pti;

pub use pti::{COEX_EVENT_COUNT, CoexEventId, CoexPti, CoexPtiTable};
