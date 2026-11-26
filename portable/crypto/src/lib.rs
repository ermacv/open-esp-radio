#![cfg_attr(
    test,
    expect(
        clippy::cast_possible_truncation,
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

//! The AES-128 key owner, the engine contract and the mode algorithms over it.
//!
//! An operation is a function over what its caller lends it for the call: the
//! engine ([`block::Aes128Engine`]), the key ([`Aes128Key`]) and the buffer
//! it works in place. [`ccm`], [`cmac`] and [`rfc3394`] each hold the one copy
//! of their algorithm, over a block engine that may stop
//! ([`block::BlockEngine`]); a key unwrap answers its plaintext as a slice
//! of the buffer, and a refusal clears what it must.

#[cfg(test)]
extern crate std;

pub mod block;
pub mod ccm;
pub mod cmac;
mod compare;
#[cfg(any(test, feature = "test-support"))]
pub mod conformance;
pub mod rfc3394;
#[cfg(test)]
mod test_engine;

use zeroize::{Zeroize, ZeroizeOnDrop};

/// An AES-128 key, erased when dropped.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Aes128Key([u8; 16]);

impl Aes128Key {
    /// Copy key material into an owner that erases it on drop.
    pub const fn copy_from(bytes: &[u8; 16]) -> Self {
        Self(*bytes)
    }

    /// The key material, for a block primitive loading it.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

#[cfg(test)]
mod tests;
