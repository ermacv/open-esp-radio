#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
// A forgotten owner never releases what it holds, such as an arbiter lease,
// so radio ownership must end in `Drop`.
#![deny(clippy::mem_forget)]

//! ESP32-S31 register transactions and affine radio ownership.
//!
//! [`root`] owns the protocol-neutral radio root and composes the restricted
//! PAC partitions into exclusive protocol routes. [`shared_radio`] arbitrates
//! the shared radio partitions and PHY state between concurrently running
//! routes through non-blocking leases. [`owner`] holds the running Wi-Fi MAC
//! owner and grants bounded capabilities; [`ieee80211::client`] brings Wi-Fi up
//! as a client of the arbiter.
//! [`phy`], [`ieee80211`], [`bluetooth`] and [`ieee802154`] implement domain
//! operations through those capabilities. [`types`] exposes value contracts
//! without granting PAC access. Protocol policy and executor waits belong to
//! their respective protocol and runtime crates.

#[cfg(test)]
extern crate std;

use core::future::Future;

use oer_esp32s31_pac::{
    MacInterruptRegisters as PacMacInterruptRegisters, MacInterruptSetup as PacMacInterruptSetup,
    MacPowerInterruptRegisters as PacMacPowerInterruptRegisters, RadioPhyRegisters,
};

pub mod bluetooth;

mod clock;

mod coex_participation;

pub mod coex;

pub mod power;
pub mod root;
pub mod shared_radio;
pub mod types;
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;

pub mod owner;

pub(crate) use owner::{phy_pac, phy_pac_mut, sealed};

pub mod ieee80211;
pub mod ieee802154;
pub mod phy;
