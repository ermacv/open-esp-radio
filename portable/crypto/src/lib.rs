#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::arithmetic_side_effects
    )
)]

//! Owned cryptographic operation contracts and the mode algorithms behind them.
//!
//! One operation is one request: [`KeyUnwrapRequest`] owns its key-encryption
//! key and its buffer; [`block::unwrap_now`] runs it to completion over a
//! synchronous AES-128 engine the caller lends ([`block::Aes128Engine`]) and
//! returns the [`KeyUnwrapResponse`], which carries the same buffer back. The
//! key-encryption key is erased when the request is consumed.
//!
//! The RFC 3394 algorithm has one copy, in [`rfc3394`]: the unwrap and the
//! wrap run to completion in place in the caller's buffer, each block finished
//! on return. The unwrap runs over a block engine that may refuse a key or a
//! block with a typed error ([`block::BlockEngine`]); the wrap over the
//! infallible [`block::BlockCipher`].

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

/// Key data that does not fit its fixed capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyDataCapacityError;

/// Bounded secret bytes of at most `N`: a wrapped or unwrapped key. The whole
/// buffer is erased when dropped.
pub struct KeyData<const N: usize> {
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> KeyData<N> {
    /// Copy `bytes` into a buffer of capacity `N`.
    pub fn try_copy(bytes: &[u8]) -> Result<Self, KeyDataCapacityError> {
        if bytes.len() > N {
            return Err(KeyDataCapacityError);
        }
        let mut data = Self {
            len: bytes.len(),
            bytes: [0; N],
        };
        for (out, byte) in data.bytes.iter_mut().zip(bytes) {
            *out = *byte;
        }
        Ok(data)
    }

    /// The held bytes.
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or(&[])
    }

    fn cleared() -> Self {
        Self {
            len: 0,
            bytes: [0; N],
        }
    }
}

impl<const N: usize> Zeroize for KeyData<N> {
    fn zeroize(&mut self) {
        self.len = 0;
        self.bytes.zeroize();
    }
}

impl<const N: usize> Drop for KeyData<N> {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl<const N: usize> ZeroizeOnDrop for KeyData<N> {}

/// Why an RFC 3394 unwrap refused its input or its block engine failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyUnwrapError {
    /// The wrapped data is not 64-bit blocks with at least two blocks of
    /// key data (RFC 3394 §2).
    InvalidLength,
    /// The initial value did not verify (RFC 3394 §2.2.3).
    IntegrityCheckFailed,
    /// A block engine failed; the plaintext and intermediate state are erased.
    Engine(block::BlockError),
}

/// One RFC 3394 unwrap under an AES-128 key-encryption key. The request owns
/// both; consuming it erases the key.
pub struct KeyUnwrapRequest<const N: usize> {
    kek: Aes128Key,
    wrapped: KeyData<N>,
}

impl<const N: usize> KeyUnwrapRequest<N> {
    pub const fn new(kek: Aes128Key, wrapped: KeyData<N>) -> Self {
        Self { kek, wrapped }
    }

    /// The wrapped key data.
    pub fn wrapped(&self) -> &[u8] {
        self.wrapped.as_bytes()
    }

    /// Split the request for an engine: the key goes to its block primitive,
    /// the buffer to [`rfc3394::unwrap`].
    pub fn into_parts(self) -> (Aes128Key, KeyData<N>) {
        (self.kek, self.wrapped)
    }
}

/// The completion of one unwrap: the request's buffer, holding the plaintext
/// key data on success and cleared on refusal. Only [`rfc3394`] builds a
/// success, so every plaintext passed the same integrity check.
pub struct KeyUnwrapResponse<const N: usize> {
    data: KeyData<N>,
    outcome: Result<(), KeyUnwrapError>,
}

impl<const N: usize> KeyUnwrapResponse<N> {
    /// The plaintext key data, or why the unwrap refused.
    pub fn plaintext(&self) -> Result<&[u8], KeyUnwrapError> {
        self.outcome.map(|()| self.data.as_bytes())
    }

    /// The buffer with its plaintext, or why the unwrap refused (the cleared
    /// buffer is erased with the response).
    pub fn into_plaintext(self) -> Result<KeyData<N>, KeyUnwrapError> {
        let Self { data, outcome } = self;
        outcome.map(|()| data)
    }
}

#[cfg(test)]
mod tests;
