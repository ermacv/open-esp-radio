//! Allocation-free RFC 3394 AES-128 key wrap and unwrap for WPA2 key data.
//!
//! Received key data reaches the unwrap; nothing here panics on it.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use core::future::Future;

use ::aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray},
};
use oer_wire::PrefixLength;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const RSN_KEY_DATA_CAPACITY: usize = 512;
pub const RSN_UNWRAPPED_KEY_DATA_CAPACITY: usize = RSN_KEY_DATA_CAPACITY - 8;

const RFC3394_IV: [u8; 8] = [0xa6; 8];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoftwareAesKeyUnwrapError {
    InvalidLength,
    CapacityExceeded,
    IntegrityCheckFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoftwareAesKeyWrapError {
    InvalidLength,
    CapacityExceeded,
}

/// Fixed, secret-bearing RFC 3394 ciphertext produced for an AP EAPOL-Key
/// frame. The complete buffer is cleared on drop.
pub struct RsnWrappedKeyData {
    len: PrefixLength<RSN_KEY_DATA_CAPACITY>,
    bytes: [u8; RSN_KEY_DATA_CAPACITY],
}

impl RsnWrappedKeyData {
    pub fn as_bytes(&self) -> &[u8] {
        self.len.borrow(&self.bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnUnwrappedKeyDataCapacityError;

/// Fixed owned plaintext returned by an async key-unwrap backend.
pub struct RsnUnwrappedKeyData {
    len: PrefixLength<RSN_UNWRAPPED_KEY_DATA_CAPACITY>,
    bytes: [u8; RSN_UNWRAPPED_KEY_DATA_CAPACITY],
}

impl RsnUnwrappedKeyData {
    /// Copy plaintext produced by an external or test key-unwrap backend.
    ///
    /// The public constructor is required for implementations of
    /// [`AsyncRsnKeyUnwrap`] outside this crate; keeping the fields private
    /// still prevents an invalid length from being fabricated.
    pub fn try_copy(bytes: &[u8]) -> Result<Self, RsnUnwrappedKeyDataCapacityError> {
        let mut owned = Self {
            len: PrefixLength::new(bytes.len()).ok_or(RsnUnwrappedKeyDataCapacityError)?,
            bytes: [0; RSN_UNWRAPPED_KEY_DATA_CAPACITY],
        };
        owned
            .len
            .borrow_mut(&mut owned.bytes)
            .iter_mut()
            .zip(bytes)
            .for_each(|(out, byte)| *out = *byte);
        Ok(owned)
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.len.borrow(&self.bytes)
    }
}

/// Async-capable boundary for the AES operation needed by WPA2 Message 3 and
/// Group Message 1.
///
/// The supplicant never awaits it: it returns a key-data unwrap request and a
/// driver awaits this port with the request's KEK and wrapped key data. A
/// hardware backend may resume from an interrupt. Implementations must not
/// retain either borrowed input after the returned future completes.
pub trait AsyncRsnKeyUnwrap {
    type Error;

    fn unwrap_key_data<'a>(
        &'a mut self,
        kek: &'a [u8; 16],
        encrypted: &'a [u8],
    ) -> impl Future<Output = Result<RsnUnwrappedKeyData, Self::Error>> + 'a;
}

/// Pure RustCrypto AES leaf with an explicit RFC 3394 input bound.
///
/// The returned future is ready on its first poll and performs no allocation,
/// hardware-status polling, delay, or OS call.
pub struct RsnSoftwareAes;

impl RsnSoftwareAes {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for RsnSoftwareAes {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncRsnKeyUnwrap for RsnSoftwareAes {
    type Error = SoftwareAesKeyUnwrapError;

    fn unwrap_key_data<'a>(
        &'a mut self,
        kek: &'a [u8; 16],
        encrypted: &'a [u8],
    ) -> impl Future<Output = Result<RsnUnwrappedKeyData, Self::Error>> + 'a {
        core::future::ready(software_aes128_key_unwrap(kek, encrypted))
    }
}

/// Wrap a bounded WPA2 key-data plaintext using RFC 3394 and AES-128.
///
/// WPA2 Message 3 key data is already padded to a multiple of eight bytes by
/// the frame builder. Inputs outside the RFC 3394 domain fail before output
/// is exposed.
pub fn software_aes128_key_wrap(
    kek: &[u8; 16],
    plaintext: &[u8],
) -> Result<RsnWrappedKeyData, SoftwareAesKeyWrapError> {
    if plaintext.len() < 16 || !plaintext.len().is_multiple_of(8) {
        return Err(SoftwareAesKeyWrapError::InvalidLength);
    }
    let len = plaintext
        .len()
        .checked_add(8)
        .ok_or(SoftwareAesKeyWrapError::CapacityExceeded)?;
    if len > RSN_KEY_DATA_CAPACITY {
        return Err(SoftwareAesKeyWrapError::CapacityExceeded);
    }

    let cipher = Aes128::new(&GenericArray::from(*kek));
    let mut output = RsnWrappedKeyData {
        len: PrefixLength::new(len).ok_or(SoftwareAesKeyWrapError::CapacityExceeded)?,
        bytes: [0; RSN_KEY_DATA_CAPACITY],
    };
    let (accumulator, data) = output
        .bytes
        .get_mut(..len)
        .and_then(|bytes| bytes.split_first_chunk_mut::<8>())
        .ok_or(SoftwareAesKeyWrapError::CapacityExceeded)?;
    *accumulator = RFC3394_IV;
    for (out, byte) in data.iter_mut().zip(plaintext) {
        *out = *byte;
    }
    let blocks = data.len() as u64 / 8;
    let mut block = [0; 16];
    for round in 0..=5_u64 {
        let mut remaining = &mut *data;
        let mut index = 0_u64;
        while let Some((chunk, rest)) = remaining.split_first_chunk_mut::<8>() {
            remaining = rest;
            index = index
                .checked_add(1)
                .ok_or(SoftwareAesKeyWrapError::CapacityExceeded)?;
            let [high @ .., l0, l1, l2, l3, l4, l5, l6, l7] = &mut block;
            let low = [l0, l1, l2, l3, l4, l5, l6, l7];
            for (out, byte) in high.iter_mut().zip(accumulator.iter()) {
                *out = *byte;
            }
            for (out, byte) in low.into_iter().zip(chunk.iter()) {
                *out = *byte;
            }
            encrypt(&cipher, &mut block);
            let counter = round
                .checked_mul(blocks)
                .and_then(|base| base.checked_add(index))
                .ok_or(SoftwareAesKeyWrapError::CapacityExceeded)?
                .to_be_bytes();
            let [h0, h1, h2, h3, h4, h5, h6, h7, low @ ..] = &block;
            let high = [h0, h1, h2, h3, h4, h5, h6, h7];
            for ((out, value), count) in accumulator.iter_mut().zip(high).zip(counter) {
                *out = *value ^ count;
            }
            *chunk = *low;
        }
    }
    block.zeroize();
    Ok(output)
}

fn encrypt(cipher: &Aes128, block: &mut [u8; 16]) {
    let mut array = GenericArray::from(*block);
    cipher.encrypt_block(&mut array);
    *block = array.into();
    array.zeroize();
}

fn decrypt(cipher: &Aes128, block: &mut [u8; 16]) {
    let mut array = GenericArray::from(*block);
    cipher.decrypt_block(&mut array);
    *block = array.into();
    array.zeroize();
}

pub fn software_aes128_key_unwrap(
    kek: &[u8; 16],
    encrypted: &[u8],
) -> Result<RsnUnwrappedKeyData, SoftwareAesKeyUnwrapError> {
    let Some((&first, wrapped)) = encrypted.split_first_chunk::<8>() else {
        return Err(SoftwareAesKeyUnwrapError::InvalidLength);
    };
    if wrapped.len() < 16 || !wrapped.len().is_multiple_of(8) {
        return Err(SoftwareAesKeyUnwrapError::InvalidLength);
    }
    let len = wrapped.len();
    let cipher = Aes128::new(&GenericArray::from(*kek));
    let mut accumulator = first;
    let mut output = RsnUnwrappedKeyData {
        len: PrefixLength::new(len).ok_or(SoftwareAesKeyUnwrapError::CapacityExceeded)?,
        bytes: [0; RSN_UNWRAPPED_KEY_DATA_CAPACITY],
    };
    let data = output.len.borrow_mut(&mut output.bytes);
    for (out, byte) in data.iter_mut().zip(wrapped) {
        *out = *byte;
    }
    let blocks = len as u64 / 8;
    let mut block = [0; 16];
    for round in (0..=5_u64).rev() {
        let mut remaining = &mut *data;
        let mut index = blocks;
        while let Some((rest, chunk)) = remaining.split_last_chunk_mut::<8>() {
            remaining = rest;
            let counter = round
                .checked_mul(blocks)
                .and_then(|base| base.checked_add(index))
                .ok_or(SoftwareAesKeyUnwrapError::CapacityExceeded)?
                .to_be_bytes();
            let [high @ .., l0, l1, l2, l3, l4, l5, l6, l7] = &mut block;
            let low = [l0, l1, l2, l3, l4, l5, l6, l7];
            for ((out, value), count) in high.iter_mut().zip(accumulator).zip(counter) {
                *out = value ^ count;
            }
            for (out, byte) in low.into_iter().zip(chunk.iter()) {
                *out = *byte;
            }
            decrypt(&cipher, &mut block);
            let [h0, h1, h2, h3, h4, h5, h6, h7, low @ ..] = &block;
            let high = [h0, h1, h2, h3, h4, h5, h6, h7];
            for (out, byte) in accumulator.iter_mut().zip(high) {
                *out = *byte;
            }
            *chunk = *low;
            index = index
                .checked_sub(1)
                .ok_or(SoftwareAesKeyUnwrapError::InvalidLength)?;
        }
    }
    block.zeroize();
    let mut difference = 0;
    for (actual, expected) in accumulator.iter().zip(RFC3394_IV) {
        difference |= actual ^ expected;
    }
    accumulator.zeroize();
    if difference != 0 {
        return Err(SoftwareAesKeyUnwrapError::IntegrityCheckFailed);
    }
    Ok(output)
}

#[cfg(test)]
mod tests;

impl Zeroize for RsnWrappedKeyData {
    fn zeroize(&mut self) {
        self.len = PrefixLength::ZERO;
        self.bytes.zeroize();
    }
}
impl Drop for RsnWrappedKeyData {
    fn drop(&mut self) {
        self.zeroize();
    }
}
impl ZeroizeOnDrop for RsnWrappedKeyData {}

impl Zeroize for RsnUnwrappedKeyData {
    fn zeroize(&mut self) {
        self.len = PrefixLength::ZERO;
        self.bytes.zeroize();
    }
}
impl Drop for RsnUnwrappedKeyData {
    fn drop(&mut self) {
        self.zeroize();
    }
}
impl ZeroizeOnDrop for RsnUnwrappedKeyData {}
