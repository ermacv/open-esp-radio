#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::disallowed_macros))]

//! The RustCrypto AES-128 cipher as a synchronous AES-128 engine
//! (`oer_portable_crypto::block::Aes128Engine`): [`SoftwareAes128`]. It
//! cannot stop (its `Stop` is `Infallible`), so its callers have no engine
//! error path. A composition that has no accelerator, or a protocol's own
//! software step, chooses it; nothing answers an accelerator's stop with it.

use ::aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray},
};
use oer_portable_crypto::{
    Aes128Key,
    block::{Aes128Engine, Block, BlockEngine},
};
use zeroize::Zeroize;

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
mod tests {
    use super::*;

    #[test]
    fn the_software_engine_passes_the_key_unwrap_conformance_cases() {
        oer_portable_crypto::conformance::key_unwrap_rfc3394(&mut SoftwareAes128);
    }

    /// FIPS-197 Appendix C.1: AES-128 one way and back.
    #[test]
    fn a_block_encrypts_and_decrypts_the_fips197_vector() {
        let key = Aes128Key::copy_from(&[
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ]);
        let plaintext = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let ciphertext = [
            0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4,
            0xc5, 0x5a,
        ];
        let Ok(mut keyed) = SoftwareAes128.load(&key);
        let mut block = plaintext;
        let Ok(()) = keyed.encrypt(&mut block);
        assert_eq!(block, ciphertext);
        let Ok(()) = keyed.decrypt(&mut block);
        assert_eq!(block, plaintext);
    }

    #[test]
    fn its_key_schedule_is_erased_on_drop() {
        fn erased<T: zeroize::ZeroizeOnDrop>() {}
        erased::<Aes128>();
        erased::<::aes::Aes192>();
        erased::<::aes::Aes256>();
    }
}
