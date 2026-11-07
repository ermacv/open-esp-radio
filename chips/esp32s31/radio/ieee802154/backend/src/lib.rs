#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
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
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::arithmetic_side_effects,
        clippy::unreachable
    )
)]
//! ESP32-S31 IEEE 802.15.4 driver above the register and DMA HAL.
//! The driver owns the engine adapter and its placed MAC epoch; HAL owns
//! register atoms, affine startup phases and permanent DMA projection.
#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod engine;
pub mod ll;
pub mod tx_power;

pub mod lifecycle;
