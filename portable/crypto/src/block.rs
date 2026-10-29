//! The synchronous AES-128 block primitive under the mode algorithms, and the
//! engine of a primitive that completes each block at once.

use crate::{
    Aes128Key, KeyUnwrap, KeyUnwrapDone, KeyUnwrapRequest, KeyUnwrapResponse,
    rfc3394::{Rfc3394Step, Rfc3394Unwrap},
};

/// One 128-bit cipher block.
pub type Block = [u8; 16];

/// Secret input or output of one block operation, erased when dropped.
#[derive(zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
pub struct CipherBlock(Block);

impl CipherBlock {
    pub const fn new(bytes: Block) -> Self {
        Self(bytes)
    }
    pub const fn as_bytes(&self) -> &Block {
        &self.0
    }
    pub fn as_mut_bytes(&mut self) -> &mut Block {
        &mut self.0
    }
}

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

/// The [`KeyUnwrap`] engine of a synchronous primitive: the operation is
/// ready on its first poll.
pub struct BlockKeyUnwrap<P> {
    blocks: P,
}

impl<P: Aes128Blocks> BlockKeyUnwrap<P> {
    pub const fn new(blocks: P) -> Self {
        Self { blocks }
    }

    pub fn into_blocks(self) -> P {
        self.blocks
    }

    /// Unwrap `request` to completion. The key is erased from the request
    /// before the first block and from the primitive after the last.
    pub fn unwrap_now<const N: usize>(
        &mut self,
        request: KeyUnwrapRequest<N>,
    ) -> KeyUnwrapResponse<N> {
        let (kek, wrapped) = request.into_parts();
        let machine = match Rfc3394Unwrap::start(wrapped) {
            Ok(machine) => machine,
            Err(refused) => return refused,
        };
        let mut cipher = self.blocks.load_key(&kek);
        drop(kek);
        let mut step = machine.next_block();
        loop {
            step = match step {
                Rfc3394Step::Block { mut input, pending } => {
                    cipher.decrypt_block(input.as_mut_bytes());
                    pending.complete(Ok(input))
                }
                Rfc3394Step::Complete(response) => return response,
            };
        }
    }
}

impl<P: Aes128Blocks, const N: usize> KeyUnwrap<N> for BlockKeyUnwrap<P> {
    type Operation = core::future::Ready<KeyUnwrapDone<Self, N>>;

    fn unwrap_key(mut self, request: KeyUnwrapRequest<N>) -> Self::Operation {
        let response = self.unwrap_now(request);
        core::future::ready(KeyUnwrapDone {
            engine: self,
            response,
        })
    }
}
