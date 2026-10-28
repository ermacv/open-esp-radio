#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::expect_used,
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    clippy::unwrap_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::arithmetic_side_effects
    )
)]

//! Stateless IEEE 802.11 frame protection and bounded AES key wrapping.
//! Packet-number allocation, replay frontiers and key negotiation belong to
//! the cores; callers supply key material and packet numbers to these transforms.

#[cfg(test)]
extern crate std;

pub mod aes;
pub mod bip;
mod ccm;
pub mod ccmp_mpdu;
pub mod management_ccmp;

#[cfg(test)]
mod key_schedule_tests {
    #[test]
    fn aes_key_schedules_zeroize_on_drop() {
        fn assert_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
        assert_zeroize_on_drop::<aes::Aes128>();
        assert_zeroize_on_drop::<aes::Aes192>();
        assert_zeroize_on_drop::<aes::Aes256>();
    }
}
