//! ESP32-S31 IEEE 802.15.4 driver above the register and DMA HAL.
//! The driver owns the engine adapter and its placed MAC epoch; HAL owns
//! register transactions, startup discard and permanent DMA projection.
#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod engine;
pub mod ll;
pub mod tx_power;
