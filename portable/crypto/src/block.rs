//! The synchronous AES-128 engine under the mode algorithms: one pair of
//! traits, the engine ([`Aes128Engine`]) and its keyed view
//! ([`BlockEngine`]), each block finished on return. An engine names what a
//! block may answer besides its result: nothing for software
//! ([`core::convert::Infallible`]), [`EngineStopped`] for an engine that may
//! stop.

use crate::{Aes128Key, KeyUnwrapRequest, KeyUnwrapResponse, rfc3394};

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

/// Wrap `request` to completion over `engine` ([`rfc3394::wrap`]): each
/// block is encrypted at once, and a refused key or block ends the operation
/// with [`crate::rfc3394::KeyWrapError::Engine`]. The key is erased from the
/// request right after the engine loads it, before the first block, and from
/// the engine after the last; plaintext of an invalid length loads no key.
pub fn wrap_now<E: Aes128Engine, const N: usize>(
    engine: &mut E,
    request: crate::KeyWrapRequest<N>,
) -> crate::KeyWrapResponse<N, E::Stop> {
    let (kek, plaintext) = request.into_parts();
    crate::rfc3394::wrap(plaintext, move || engine.load(&kek))
}

/// Unwrap `request` to completion over `engine` ([`rfc3394::unwrap`]): each
/// block is decrypted at once, and a refused key or block ends the operation
/// with [`crate::KeyUnwrapError::Engine`]. The key is erased from the request
/// right after the engine loads it, before the first block, and from the
/// engine after the last; wrapped data of an invalid length loads no key.
pub fn unwrap_now<E: Aes128Engine, const N: usize>(
    engine: &mut E,
    request: KeyUnwrapRequest<N>,
) -> KeyUnwrapResponse<N, E::Stop> {
    let (kek, wrapped) = request.into_parts();
    rfc3394::unwrap(wrapped, move || engine.load(&kek))
}
