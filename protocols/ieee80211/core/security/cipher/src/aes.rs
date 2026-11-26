//! The software AES-128 block primitive and the RSN key-data bound over the
//! RFC 3394 algorithm of `oer-portable-crypto`.
//!
//! [`SoftwareAes128`] implements `oer_portable_crypto::block::Aes128Engine` with
//! the RustCrypto cipher. It cannot stop (its `Stop` is `Infallible`), so its
//! callers have no engine error path. WPA2 key data is bounded by [`RSN_KEY_DATA_CAPACITY`].

use ::aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray},
};
use oer_portable_crypto::{
    Aes128Key, KeyData,
    block::{Aes128Engine, Block, BlockEngine},
};
use zeroize::Zeroize;

/// The largest wrapped WPA2 Key Data field the station and access point take.
pub const RSN_KEY_DATA_CAPACITY: usize = 512;

/// Wrapped or unwrapped WPA2 key data, erased on drop.
pub type RsnKeyData = KeyData<RSN_KEY_DATA_CAPACITY>;

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
