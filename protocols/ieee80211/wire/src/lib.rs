#![no_std]
#![forbid(unsafe_code)]

//! IEEE 802.11 frame formats and shared value vocabulary.
//!
//! This crate owns bounded frame parsing, encoding and protocol values. It has no
//! MMIO, DMA, interrupt, executor, allocator, ESP32-S31, vendor archive, or
//! ROM ABI dependency.
//!
//! [`address::Bssid`] proves a nonzero individual address.
//! [`scan::ScanRecord::new`] requires the actual BSSID, channel and RSSI.
//! The station core owns the BSS observation table.
//!
//! Management and A-MSDU construction retain immutable frame parts through
//! capacity admission and writing. Those same parts determine the output
//! length; no independent final-offset assertion protects the layout.
//! In-place station encapsulation borrows prefix and payload separately so
//! writing a new prefix cannot overwrite the retained Ethernet payload.
//!
//! The strict-lint parser contract requires field boundaries established
//! with the bounded `oer_wire::Reader` and retained as fixed-size arrays through
//! subsequent processing. Replacing indexing mechanically with `.get()` or
//! suppressing its lint does not establish the contract.

#[cfg(any(test, feature = "test-support"))]
extern crate std;

/// Deterministic received-byte inputs, shared with other crates' tests
/// through the `test-support` feature.
#[cfg(any(test, feature = "test-support"))]
pub mod fuzz;

pub mod address;
pub mod ampdu;
pub mod ap;
pub mod beacon;
pub mod block_ack;
pub mod ccmp;
pub mod channel;
pub mod channel_switch;
pub mod classification;
pub mod data;
mod encoding;
pub mod extensions;
pub mod qos;

pub mod fragmentation;
pub mod ftm;
pub mod he;
pub mod ht;
pub mod management;
pub mod management_protection;
pub mod ndpa;
pub mod phy;
pub mod protection;
pub mod scan;
pub mod security;
pub mod sequence;
pub mod signal;
pub mod ssid;
pub mod station;
pub mod station_beacon;
pub mod station_power_save;
pub mod tbtt;
pub mod trigger;
pub mod tsf;
pub mod twt;
pub mod vif;

/// Checked management/data headers and receive admission.
pub mod frame;
