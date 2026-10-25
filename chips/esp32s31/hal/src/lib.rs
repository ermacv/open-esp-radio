#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
// A forgotten owner never releases what it holds, such as a modem clock lease,
// so radio ownership must end in `Drop`.
#![deny(clippy::mem_forget)]

//! ESP32-S31 register transactions and affine radio ownership.
//!
//! [`root`] claims the chip once and splits the PAC's partitions by domain:
//! the bootstrap's owners ([`boot`]), the platform ([`platform`], [`cores`],
//! [`watchdog`], [`usb_serial_jtag`], [`executor`]) and the protocol-neutral
//! radio root, which composes the radio partitions into exclusive protocol
//! routes. [`domain`] holds the shared
//! radio partitions and PHY state of concurrently running protocols, owned by
//! value by the radio system. [`owner`] holds the running Wi-Fi MAC owner and
//! grants bounded capabilities; [`ieee80211::client`] brings Wi-Fi up as a
//! client of the radio domain.
//! [`phy`], [`ieee80211`], [`bluetooth`] and [`ieee802154`] implement domain
//! operations through those capabilities. [`types`] exposes value contracts
//! without granting PAC access. Protocol policy and executor waits belong to
//! their respective protocol and runtime crates.

#[cfg(test)]
extern crate std;

use core::future::Future;

use crate::ieee80211::registers::{
    MacInterruptRegisters as PacMacInterruptRegisters, MacInterruptSetup as PacMacInterruptSetup,
    MacPowerInterruptRegisters as PacMacPowerInterruptRegisters,
};
use crate::phy::RadioPhyRegisters;

pub mod bluetooth;
pub mod boot;

mod clock;

mod coex_participation;

pub mod coex;

pub mod cores;

pub mod domain;
pub mod executor;
pub mod modem;
pub mod power;
pub mod root;
pub mod types;

pub mod owner;

#[cfg(target_arch = "riscv32")]
pub(crate) use owner::phy_pac_mut;
pub(crate) use owner::{phy_blocks, phy_blocks_mut, sealed};

pub mod ieee80211;
pub mod ieee802154;

/// The memory layouts the radio hardware reads and writes in RAM, from the
/// PAC: the protocol layers above reach them through these types only.
pub use oer_esp32s31_pac::layouts;
pub mod dma_address;
pub mod dma_static;
pub mod phy;
pub mod platform;
pub mod usb_serial_jtag;
/// Bridges of isolated probe images (`cfg(oer_probe)`).
#[cfg(oer_probe)]
#[doc(hidden)]
pub mod validation;
pub mod watchdog;

use phy::registers::RadioPhyRegisters;
