#![cfg_attr(
    test,
    expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::let_underscore_must_use,
        reason = "test code: a cast or a dropped result in a test loses no product's value"
    )
)]
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

//! IEEE 802.11 frame formats and shared value vocabulary.
//!
//! This crate owns bounded frame parsing, encoding and protocol values. It has no
//! MMIO, DMA, interrupt, executor, allocator, ESP32-S31, vendor archive, or
//! ROM ABI dependency.
//!
//! [`address::Bssid`] proves a nonzero individual address.
//! [`scan::ScanRecord::new`] requires the actual BSSID; the channel a BSS is
//! on is the station's decision, from its DS Parameter Set or the receiver.
//! The station core owns the BSS observation table.
//!
//! Management and A-MSDU construction retain immutable frame parts through
//! capacity admission and writing. Those same parts determine the output
//! length; no independent final-offset assertion protects the layout.
//! In-place station encapsulation retains the complete frame borrow. Its
//! immutable prefix parts define all bytes written, preserving the payload.
//!
//! The strict-lint parser contract requires field boundaries established
//! with the bounded `oer_portable_wire::Reader` and retained as fixed-size arrays through
//! subsequent processing. Replacing indexing mechanically with `.get()` or
//! suppressing its lint does not establish the contract.

#[cfg(any(test, feature = "test-support"))]
extern crate std;

/// Deterministic received-byte inputs, shared with other crates' tests
/// through the `test-support` feature.
#[cfg(any(test, feature = "test-support"))]
pub mod fuzz;

pub mod action;
pub mod address;
pub mod ampdu;
pub mod ap;
pub mod beacon;
pub mod bip;
pub mod block_ack;
pub mod ccmp;
pub mod channel;
pub mod channel_switch;
mod code;
pub mod data;
pub mod eapol;
pub mod element;
mod encoding;
pub mod extensions;
pub mod qos;

pub mod fragmentation;
pub mod ftm;
pub mod he;
pub mod ht;
pub mod management;
pub mod management_protection;
pub mod monitor_injection;
pub mod phy;
pub mod protection;
pub mod rates;
pub mod sae;
pub mod scan;
pub mod security;
pub mod sequence;
pub mod signal;
pub mod ssid;
pub mod station;
pub mod station_beacon;
pub mod station_power_save;
pub mod status;
pub mod tsf;
pub mod twt;

/// Checked management/data headers and receive admission.
pub mod frame;

/// Independent protected-management wire fixture for host tests.
#[cfg(any(test, feature = "test-support"))]
pub mod management_ccmp_vector;
