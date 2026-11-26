//! The synchronous AES-128 engine under the mode algorithms: one pair of
//! traits, the engine ([`Aes128Engine`]) and its keyed view
//! ([`BlockEngine`]), each block finished on return. An engine names what a
//! block may answer besides its result: nothing for software
//! ([`core::convert::Infallible`]), [`EngineStopped`] for an engine that may
//! stop.

use crate::Aes128Key;

/// One 128-bit cipher block.
pub type Block = [u8; 16];

/// The one refusal of an engine that may stop: it stopped, mid-operation or
/// before it. It carries nothing: what the engine observed is its driver's
/// record, which the driver's holder reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineStopped;

/// A keyed block engine: one block in place per call, finished on return,
/// or the engine's [`Self::Stop`]. A software engine's is
/// [`core::convert::Infallible`], so its callers have no error path.
pub trait BlockEngine {
    /// What a block may answer besides its result.
    type Stop: Copy;

    fn encrypt(&mut self, block: &mut Block) -> Result<(), Self::Stop>;
    fn decrypt(&mut self, block: &mut Block) -> Result<(), Self::Stop>;
}

/// An AES-128 block engine that loads a key for a run of blocks.
pub trait Aes128Engine {
    /// What loading a key or a block may answer besides its result.
    type Stop: Copy;

    /// The engine with its key loaded. Dropping it erases the key from the
    /// engine.
    type Keyed<'a>: BlockEngine<Stop = Self::Stop>
    where
        Self: 'a;

    fn load(&mut self, key: &Aes128Key) -> Result<Self::Keyed<'_>, Self::Stop>;
}
