#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

//! Hardware-independent Bluetooth Low Energy Link Layer building blocks.
//!
//! This crate owns connection/channel progression, duplicate filtering,
//! control and data-length procedures, encryption state and AES-CCM transforms.
//! Over-the-air fields and codecs live in `oer-bluetooth-le-wire`; shared DTM
//! window geometry belongs to `oer-bluetooth-le-port`. A Controller composes
//! these pieces without HCI, MMIO, DMA or executor dependencies in this core.

#[cfg(test)]
extern crate std;

pub mod connection;
pub mod control;
pub mod data_length;
pub mod scanning;
pub mod security;

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
