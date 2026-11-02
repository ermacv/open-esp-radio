//! The synchronous AES-128 block primitives under the mode algorithms, and the
//! engine of a primitive that completes each block at once: an infallible
//! one ([`Aes128Blocks`], software) and one that may refuse a key or a block
//! ([`Aes128Engine`], hardware), which every infallible one also is.

use crate::{Aes128Key, KeyUnwrap, KeyUnwrapDone, KeyUnwrapRequest, KeyUnwrapResponse, rfc3394};

/// One 128-bit cipher block.
pub type Block = [u8; 16];

/// A block engine's refusal, independent of the mode's input errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockError {
    /// The engine retains a failed resource and accepts no more blocks.
    Unavailable,
    /// Completion did not arrive within the engine's policy budget.
    Timeout { observation: u32 },
    /// A status outside the selected operation's domain was observed.
    InvalidState { observation: u32 },
    /// The monotonic clock cannot represent the operation's bound.
    TimeOverflow,
}

/// A keyed block cipher: one block in place per call, finished on return.
pub trait BlockCipher {
    fn encrypt_block(&mut self, block: &mut Block);
    fn decrypt_block(&mut self, block: &mut Block);
}

/// An AES-128 block primitive that loads a key for a run of blocks.
pub trait Aes128Blocks {
    /// The primitive with its key loaded. Dropping it erases the key from
    /// the primitive.
    type Keyed<'a>: BlockCipher
    where
        Self: 'a;

    fn load_key(&mut self, key: &Aes128Key) -> Self::Keyed<'_>;
}

/// A borrowed primitive is one: an engine over `&mut P` leaves `P` with its
/// owner, which runs it again for the next request.
impl<P: Aes128Blocks> Aes128Blocks for &mut P {
    type Keyed<'a>
        = P::Keyed<'a>
    where
        Self: 'a;

    fn load_key(&mut self, key: &Aes128Key) -> Self::Keyed<'_> {
        (**self).load_key(key)
    }
}

/// A keyed block engine that may refuse a block: one block in place per call,
/// finished on return, or the engine's [`BlockError`].
pub trait BlockEngine {
    fn encrypt(&mut self, block: &mut Block) -> Result<(), BlockError>;
    fn decrypt(&mut self, block: &mut Block) -> Result<(), BlockError>;
}

/// Every infallible block cipher is an engine that never refuses.
impl<C: BlockCipher> BlockEngine for C {
    fn encrypt(&mut self, block: &mut Block) -> Result<(), BlockError> {
        self.encrypt_block(block);
        Ok(())
    }
    fn decrypt(&mut self, block: &mut Block) -> Result<(), BlockError> {
        self.decrypt_block(block);
        Ok(())
    }
}

/// An AES-128 block engine that loads a key for a run of blocks and may
/// refuse the key or a block (a hardware engine's state or timeout).
pub trait Aes128Engine {
    /// The engine with its key loaded. Dropping it erases the key from the
    /// engine.
    type Keyed<'a>: BlockEngine
    where
        Self: 'a;

    fn load(&mut self, key: &Aes128Key) -> Result<Self::Keyed<'_>, BlockError>;
}

/// Every infallible primitive is an engine that never refuses.
impl<P: Aes128Blocks> Aes128Engine for P {
    type Keyed<'a>
        = P::Keyed<'a>
    where
        Self: 'a;

    fn load(&mut self, key: &Aes128Key) -> Result<Self::Keyed<'_>, BlockError> {
        Ok(self.load_key(key))
    }
}

/// Unwrap `request` to completion over `engine` ([`rfc3394::unwrap`]): each
/// block is decrypted at once, and a refused key or block ends the operation
/// with [`crate::KeyUnwrapError::Engine`]. The key is erased from the request
/// right after the engine loads it, before the first block, and from the
/// engine after the last; wrapped data of an invalid length loads no key.
pub fn unwrap_now<E: Aes128Engine, const N: usize>(
    engine: &mut E,
    request: KeyUnwrapRequest<N>,
) -> KeyUnwrapResponse<N> {
    let (kek, wrapped) = request.into_parts();
    rfc3394::unwrap(wrapped, move || engine.load(&kek))
}

/// The [`KeyUnwrap`] engine of a synchronous primitive or engine: the
/// operation is ready on its first poll.
pub struct BlockKeyUnwrap<P> {
    blocks: P,
}

impl<P: Aes128Engine> BlockKeyUnwrap<P> {
    pub const fn new(blocks: P) -> Self {
        Self { blocks }
    }

    pub fn into_blocks(self) -> P {
        self.blocks
    }

    /// Unwrap `request` to completion ([`unwrap_now`]).
    pub fn unwrap_now<const N: usize>(
        &mut self,
        request: KeyUnwrapRequest<N>,
    ) -> KeyUnwrapResponse<N> {
        unwrap_now(&mut self.blocks, request)
    }
}

impl<P: Aes128Engine, const N: usize> KeyUnwrap<N> for BlockKeyUnwrap<P> {
    type Operation = core::future::Ready<KeyUnwrapDone<Self, N>>;

    fn unwrap_key(mut self, request: KeyUnwrapRequest<N>) -> Self::Operation {
        let response = self.unwrap_now(request);
        core::future::ready(KeyUnwrapDone {
            engine: self,
            response,
        })
    }
}
