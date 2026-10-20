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

#[cfg(test)]
mod key_schedule_tests {
    #[test]
    fn aes_key_schedules_zeroize_on_drop() {
        fn assert_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
        assert_zeroize_on_drop::<super::security::LeLongTermKey>();
        assert_zeroize_on_drop::<super::security::LeAclSessionKey>();
        assert_zeroize_on_drop::<aes::Aes128>();
        assert_zeroize_on_drop::<aes::Aes192>();
        assert_zeroize_on_drop::<aes::Aes256>();
    }
}
