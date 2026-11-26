//! AES-CMAC (RFC 4493) over a keyed block engine, fed in pieces.
//!
//! [`Cmac`] owns one keyed engine for one message: it derives the subkey from
//! `L = E(K, 0^128)` when it starts, keeps the last full block back until it
//! knows whether it is the message's final block, and pads the final block
//! with `10*` under the second subkey otherwise. The first refused block
//! poisons the computation: every later call returns the same [`BlockError`].
//! Every secret block lies in the computation's own fields, where it is
//! erased: the subkeys are derived from `L` in its place and the final block
//! is padded in the held-back block's, and finishing erases all three on
//! every exit; the returned tag is the caller's.

use crate::block::{Block, BlockEngine, BlockError};
use zeroize::Zeroize;

const BLOCK: usize = 16;

/// RFC 4493 `const_Rb` for a 128-bit block.
const RB: u8 = 0x87;

/// Why a tag did not verify.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CmacVerifyError {
    /// The tag is not the message's under the key.
    InvalidTag,
    /// The engine refused a block.
    Engine(BlockError),
}

/// One AES-CMAC computation over the keyed engine `E`. `L`, the chaining
/// value and the held-back block are erased when it is dropped.
#[derive(zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
pub struct Cmac<E: BlockEngine> {
    #[zeroize(skip)]
    engine: E,
    /// `L = E(K, 0^128)`, from which both subkeys follow.
    l: Block,
    /// The CBC chaining value of the blocks already processed.
    state: Block,
    /// The bytes not yet processed: at most one block, zero beyond `filled`.
    pending: Block,
    /// How many bytes of `pending` hold message data, `0..=16`.
    #[zeroize(skip)]
    filled: usize,
    /// The first refusal of the engine.
    #[zeroize(skip)]
    failed: Option<BlockError>,
}

impl<E: BlockEngine> Cmac<E> {
    /// Start a computation over `engine`, which holds the key.
    pub fn new(mut engine: E) -> Result<Self, BlockError> {
        let mut l = [0; BLOCK];
        engine.encrypt(&mut l)?;
        Ok(Self {
            engine,
            l,
            state: [0; BLOCK],
            pending: [0; BLOCK],
            filled: 0,
            failed: None,
        })
    }

    /// Feed the next piece of the message.
    pub fn update(&mut self, bytes: &[u8]) -> Result<(), BlockError> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        let mut rest = bytes.iter();
        loop {
            // Fill the held-back block from where it stopped.
            for (slot, byte) in self.pending.iter_mut().skip(self.filled).zip(rest.by_ref()) {
                *slot = *byte;
                self.filled = self.filled.saturating_add(1);
            }
            if rest.len() == 0 {
                break;
            }
            // The held-back block is full and more follows: it is not the
            // last block.
            xor_into(&mut self.state, &self.pending);
            self.pending.zeroize();
            self.filled = 0;
            if let Err(error) = self.engine.encrypt(&mut self.state) {
                self.failed = Some(error);
                return Err(error);
            }
        }
        Ok(())
    }

    /// The full 128-bit tag.
    pub fn finalize(mut self) -> Result<Block, BlockError> {
        self.finish()
    }

    /// The tag, with `L`, the chaining value and the held-back block erased
    /// in their places on every exit.
    fn finish(&mut self) -> Result<Block, BlockError> {
        let tag = self.tag();
        self.l.zeroize();
        self.state.zeroize();
        self.pending.zeroize();
        tag
    }

    /// The tag, computed in place: `L` becomes `K1`, and `K2` when the last
    /// block is partial; the held-back block becomes the masked last block;
    /// the chaining value becomes the tag.
    fn tag(&mut self) -> Result<Block, BlockError> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        double(&mut self.l);
        // A partial last block is padded with `10*` under `K2`; a full one
        // has no byte after its data and takes `K1`.
        if let Some(pad) = self.pending.get_mut(self.filled) {
            *pad = 0x80;
            double(&mut self.l);
        }
        xor_into(&mut self.pending, &self.l);
        xor_into(&mut self.state, &self.pending);
        self.engine.encrypt(&mut self.state)?;
        Ok(self.state)
    }

    /// Compare the tag's first `T` bytes with `expected` in constant time:
    /// it verifies, or the refusal.
    pub fn verify_truncated<const T: usize>(
        self,
        expected: &[u8; T],
    ) -> Result<(), CmacVerifyError> {
        const {
            if T == 0 || T > BLOCK {
                panic!("a CMAC tag is 1 to 16 bytes");
            }
        }
        let mut tag = self.finalize().map_err(CmacVerifyError::Engine)?;
        let equal = crate::compare::constant_time_eq(tag.iter().take(T), expected.iter());
        tag.zeroize();
        if equal {
            Ok(())
        } else {
            Err(CmacVerifyError::InvalidTag)
        }
    }
}

/// The RFC 4493 subkey step, in place: shift left by one bit, then add
/// `const_Rb` when the shifted-out bit was set.
fn double(block: &mut Block) {
    let mut carry = 0;
    for byte in block.iter_mut().rev() {
        let next = *byte >> 7;
        *byte = (*byte << 1) | carry;
        carry = next;
    }
    let [.., last] = block;
    // Branch-free: `carry` is 0 or 1.
    *last ^= RB & 0_u8.wrapping_sub(carry);
}

fn xor_into(target: &mut Block, source: &Block) {
    for (out, byte) in target.iter_mut().zip(source) {
        *out ^= byte;
    }
}

#[cfg(test)]
mod tests;
