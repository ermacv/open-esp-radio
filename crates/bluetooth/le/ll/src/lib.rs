#![no_std]
#![forbid(unsafe_code)]

//! Hardware-independent Bluetooth Low Energy Link Layer building blocks.
//!
//! This crate owns bounded over-the-air PDU codecs and the protocol state of
//! one connection: advertising and scan-response PDUs, advertising report
//! parsing and duplicate filtering, connection indication, channel selection
//! and instants, control procedures, encryption and the Direct Test Mode
//! test planner. It has no HCI, MMIO, DMA, interrupt, executor, allocator,
//! ESP32-S31, vendor-archive or ROM-ABI dependency. A Controller schedules
//! roles and composes these pieces.
//!
//! [`connection::maintenance`] admits provisional budgeted event omissions on
//! the same connection owner, retaining initial acknowledgement, Instant and
//! post-maintenance recovery obligations. This protocol decision grants no
//! hardware access or timing budget; both remain with the lower radio owner.

#[cfg(test)]
extern crate std;

mod address;
pub mod advertising;
pub mod connectable_advertising;
pub mod connection;
pub mod control;
pub mod data_length;
pub mod dtm;
pub mod scanning;
pub mod security;

pub use address::{LeDeviceAddress, LeDeviceAddressKind};
