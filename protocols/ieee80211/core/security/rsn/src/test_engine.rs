//! A test engine that refuses every key, as a failed hardware engine would.

use oer_portable_crypto::{
    Aes128Key,
    block::{Aes128Engine, Block, BlockEngine, EngineStopped},
};

/// An engine that refuses every key.
pub(crate) struct Refusing;

impl Aes128Engine for Refusing {
    type Stop = EngineStopped;
    type Keyed<'a> = NeverKeyed;

    fn load(&mut self, _key: &Aes128Key) -> Result<Self::Keyed<'_>, EngineStopped> {
        Err(EngineStopped)
    }
}

/// The keyed half of [`Refusing`]; it is never created.
pub(crate) struct NeverKeyed;

impl BlockEngine for NeverKeyed {
    type Stop = EngineStopped;

    fn encrypt(&mut self, _block: &mut Block) -> Result<(), EngineStopped> {
        Err(EngineStopped)
    }
    fn decrypt(&mut self, _block: &mut Block) -> Result<(), EngineStopped> {
        Err(EngineStopped)
    }
}
