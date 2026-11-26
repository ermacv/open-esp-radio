//! Block engines of the mode tests: RustCrypto AES-128 and one that refuses
//! from a chosen block on.

use crate::block::{Block, BlockEngine, EngineStopped};
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};

pub(crate) const REFUSAL: EngineStopped = EngineStopped;

/// RustCrypto AES-128 under `key`, counting its blocks; it refuses every block
/// from `refuse_from` on.
pub(crate) struct Engine {
    cipher: aes::Aes128,
    pub(crate) blocks: usize,
    refuse_from: usize,
}

impl Engine {
    pub(crate) fn new(key: &[u8; 16]) -> Self {
        Self::refusing_from(key, usize::MAX)
    }

    pub(crate) fn refusing_from(key: &[u8; 16], refuse_from: usize) -> Self {
        Self {
            cipher: aes::Aes128::new(&GenericArray::from(*key)),
            blocks: 0,
            refuse_from,
        }
    }

    fn admit(&mut self) -> Result<(), EngineStopped> {
        if self.blocks >= self.refuse_from {
            return Err(REFUSAL);
        }
        self.blocks += 1;
        Ok(())
    }
}

impl BlockEngine for Engine {
    type Stop = EngineStopped;

    fn encrypt(&mut self, block: &mut Block) -> Result<(), EngineStopped> {
        self.admit()?;
        let mut array = GenericArray::from(*block);
        self.cipher.encrypt_block(&mut array);
        *block = array.into();
        Ok(())
    }

    fn decrypt(&mut self, block: &mut Block) -> Result<(), EngineStopped> {
        self.admit()?;
        let mut array = GenericArray::from(*block);
        self.cipher.decrypt_block(&mut array);
        *block = array.into();
        Ok(())
    }
}

impl BlockEngine for &mut Engine {
    type Stop = EngineStopped;

    fn encrypt(&mut self, block: &mut Block) -> Result<(), EngineStopped> {
        (**self).encrypt(block)
    }

    fn decrypt(&mut self, block: &mut Block) -> Result<(), EngineStopped> {
        (**self).decrypt(block)
    }
}
