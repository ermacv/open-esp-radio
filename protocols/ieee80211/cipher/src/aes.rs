//! The software AES-128 block primitive and the RSN key-data bound over the
//! RFC 3394 algorithm of `oer-crypto`.
//!
//! [`SoftwareAes128`] implements `oer_crypto::block::Aes128Blocks` with the
//! RustCrypto cipher; [`SoftwareKeyUnwrap`] immediately answers each owned
//! block request from the shared RFC 3394 machine, ready on the first poll. WPA2 key data is bounded by [`RSN_KEY_DATA_CAPACITY`].
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use ::aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray},
};
use oer_crypto::{
    Aes128Key, KeyData,
    block::{Aes128Blocks, Block, BlockCipher, BlockKeyUnwrap},
    rfc3394::KeyWrapError,
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

impl Aes128Blocks for SoftwareAes128 {
    type Keyed<'a> = SoftwareAes128Keyed;

    fn load_key(&mut self, key: &Aes128Key) -> SoftwareAes128Keyed {
        SoftwareAes128Keyed(Aes128::new(&GenericArray::from(*key.as_bytes())))
    }
}

impl BlockCipher for SoftwareAes128Keyed {
    fn encrypt_block(&mut self, block: &mut Block) {
        let mut array = GenericArray::from(*block);
        self.0.encrypt_block(&mut array);
        *block = array.into();
        array.zeroize();
    }

    fn decrypt_block(&mut self, block: &mut Block) {
        let mut array = GenericArray::from(*block);
        self.0.decrypt_block(&mut array);
        *block = array.into();
        array.zeroize();
    }
}

/// The software RFC 3394 unwrap engine: its operation is ready on the first
/// poll and performs no allocation, hardware polling, delay or OS call.
pub type SoftwareKeyUnwrap = BlockKeyUnwrap<SoftwareAes128>;

/// Wrap a bounded WPA2 key-data plaintext with RFC 3394 under an AES-128 KEK.
///
/// WPA2 Message 3 key data is already padded to a multiple of eight bytes by
/// the frame builder. Inputs outside the RFC 3394 domain fail before output
/// is exposed.
pub fn software_aes128_key_wrap(
    kek: &[u8; 16],
    plaintext: &[u8],
) -> Result<RsnKeyData, KeyWrapError> {
    let key = Aes128Key::copy_from(kek);
    let mut cipher = SoftwareAes128.load_key(&key);
    drop(key);
    oer_crypto::rfc3394::wrap(&mut cipher, plaintext)
}

#[cfg(test)]
mod tests;
