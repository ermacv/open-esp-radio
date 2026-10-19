//! Chip-neutral IEEE 802.15.4 MAC driver engine.
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/ieee802154/driver` (Copyright Espressif Systems (Shanghai) Co
//! Ltd, Apache-2.0) and modified by Vladislav Ermachenkov; see `NOTICE`.
//!
//! [`engine`] ports the public ESP-IDF driver's state machine
//! (`esp_ieee802154_dev.c`) and public API layer over the
//! [`ll::Ieee802154LowLevel`] interface, which each chip's HAL implements over
//! its own PAC. The engine names no executor, interrupt route or chip: a
//! runtime holds it with the chip's MAC owners in one critical section and
//! calls its operation entries and interrupt handler. What differs between
//! chips — register geometry, transmit-on delays, the transmit-power level
//! set, coexistence tables and the DMA-reachable memory — stays with each
//! chip's HAL.

#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![deny(missing_docs)]

#[cfg(test)]
extern crate std;

pub mod capabilities;
pub mod channel;
pub mod coex;
pub mod engine;
pub mod ll;
pub mod pib;
pub mod tx_power;
pub mod types;
