//! The software AES-128 block primitive and the RSN key-data bound over the
//! RFC 3394 algorithm of `oer-portable-crypto`.
//!
//! [`SoftwareAes128`] implements `oer_portable_crypto::block::Aes128Engine` with
//! the RustCrypto cipher. It cannot stop (its `Stop` is `Infallible`), so its
//! callers have no engine error path. WPA2 key data lies in an
//! [`RsnKeyData`] of [`RSN_KEY_DATA_CAPACITY`] bytes, which a step lends to
//! RFC 3394 to work in place.

use ::aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray},
};
use oer_portable_crypto::{
    Aes128Key,
    block::{Aes128Engine, Block, BlockEngine},
};
use zeroize::Zeroize;

/// The largest wrapped WPA2 Key Data field the station and access point take.
pub const RSN_KEY_DATA_CAPACITY: usize = 512;

/// The place of one WPA2 Key Data field, [`RSN_KEY_DATA_CAPACITY`] bytes
/// erased on drop: the step that wraps or unwraps key data owns one and
/// lends the slice the data fills to RFC 3394 (`oer_portable_crypto::rfc3394`).
#[derive(zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
pub struct RsnKeyData([u8; RSN_KEY_DATA_CAPACITY]);

/// Key data that does not fit [`RsnKeyData`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnKeyDataTooLong;

/// One semiblock: the integrity register's room before a wrap's plaintext.
const SEMIBLOCK: usize = 8;

impl RsnKeyData {
    pub const fn new() -> Self {
        Self([0; RSN_KEY_DATA_CAPACITY])
    }

    /// Copy wrapped key data in: the slice it fills, for the unwrap.
    pub fn wrapped(&mut self, bytes: &[u8]) -> Result<&mut [u8], RsnKeyDataTooLong> {
        let filled = self.0.get_mut(..bytes.len()).ok_or(RsnKeyDataTooLong)?;
        for (out, byte) in filled.iter_mut().zip(bytes) {
            *out = *byte;
        }
        Ok(filled)
    }

    /// Copy plaintext key data in after a semiblock of room: the slice of
    /// room and plaintext, for the wrap.
    pub fn plaintext(&mut self, bytes: &[u8]) -> Result<&mut [u8], RsnKeyDataTooLong> {
        let filled = bytes
            .len()
            .checked_add(SEMIBLOCK)
            .and_then(|len| self.0.get_mut(..len))
            .ok_or(RsnKeyDataTooLong)?;
        for (out, byte) in filled.iter_mut().skip(SEMIBLOCK).zip(bytes) {
            *out = *byte;
        }
        Ok(filled)
    }
}

impl Default for RsnKeyData {
    fn default() -> Self {
        Self::new()
    }
}

/// The RustCrypto AES-128 block primitive. It holds no key between runs.
pub struct SoftwareAes128;

/// [`SoftwareAes128`] with a key loaded; the key schedule is erased on drop.
pub struct SoftwareAes128Keyed(Aes128);

impl Aes128Engine for SoftwareAes128 {
    type Stop = core::convert::Infallible;
    type Keyed<'a> = SoftwareAes128Keyed;

    fn load(&mut self, key: &Aes128Key) -> Result<SoftwareAes128Keyed, Self::Stop> {
        Ok(SoftwareAes128Keyed(Aes128::new(&GenericArray::from(
            *key.as_bytes(),
        ))))
    }
}

impl BlockEngine for SoftwareAes128Keyed {
    type Stop = core::convert::Infallible;

    fn encrypt(&mut self, block: &mut Block) -> Result<(), Self::Stop> {
        let mut array = GenericArray::from(*block);
        self.0.encrypt_block(&mut array);
        *block = array.into();
        array.zeroize();
        Ok(())
    }

    fn decrypt(&mut self, block: &mut Block) -> Result<(), Self::Stop> {
        let mut array = GenericArray::from(*block);
        self.0.decrypt_block(&mut array);
        *block = array.into();
        array.zeroize();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
