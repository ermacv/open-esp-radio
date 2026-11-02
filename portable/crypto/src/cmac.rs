//! AES-CMAC (RFC 4493) over a keyed block engine, fed in pieces.
//!
//! [`Cmac`] owns one keyed engine for one message: it derives the subkey from
//! `L = E(K, 0^128)` when it starts, keeps the last full block back until it
//! knows whether it is the message's final block, and pads the final block
//! with `10*` under the second subkey otherwise. The first refused block
//! poisons the computation: every later call returns the same [`BlockError`].
//! The chaining value, the held-back block and `L` are erased on drop.

use crate::block::{Block, BlockEngine, BlockError};
use zeroize::Zeroize;

const BLOCK: usize = 16;

/// RFC 4493 `const_Rb` for a 128-bit block.
const RB: u8 = 0x87;

/// One AES-CMAC computation over the keyed engine `E`.
pub struct Cmac<E: BlockEngine> {
    engine: E,
    /// `L = E(K, 0^128)`, from which both subkeys follow.
    l: Block,
    /// The CBC chaining value of the blocks already processed.
    state: Block,
    /// The bytes not yet processed: at most one block, zero beyond `filled`.
    pending: Block,
    /// How many bytes of `pending` hold message data, `0..=16`.
    filled: usize,
    /// The first refusal of the engine.
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
        for byte in bytes {
            if self.filled >= BLOCK {
                xor_into(&mut self.state, &self.pending);
                self.pending.zeroize();
                self.filled = 0;
                if let Err(error) = self.engine.encrypt(&mut self.state) {
                    self.failed = Some(error);
                    return Err(error);
                }
            }
            if let Some(slot) = self.pending.get_mut(self.filled) {
                *slot = *byte;
            }
            self.filled = self.filled.saturating_add(1);
        }
        Ok(())
    }

    /// The full 128-bit tag.
    pub fn finalize(mut self) -> Result<Block, BlockError> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        let k1 = double(&self.l);
        let mut last = self.pending;
        if self.filled >= BLOCK {
            xor_into(&mut last, &k1);
        } else {
            let k2 = double(&k1);
            if let Some(pad) = last.get_mut(self.filled) {
                *pad = 0x80;
            }
            xor_into(&mut last, &k2);
            erase(k2);
        }
        erase(k1);
        let mut tag = self.state;
        xor_into(&mut tag, &last);
        erase(last);
        match self.engine.encrypt(&mut tag) {
            Ok(()) => Ok(tag),
            Err(error) => {
                tag.zeroize();
                Err(error)
            }
        }
    }

    /// Compare the tag's first `T` bytes with `expected` in constant time.
    pub fn verify_truncated<const T: usize>(self, expected: &[u8; T]) -> Result<bool, BlockError> {
        const {
            if T == 0 || T > BLOCK {
                panic!("a CMAC tag is 1 to 16 bytes");
            }
        }
        let mut tag = self.finalize()?;
        let equal = crate::compare::constant_time_eq(tag.iter().take(T), expected.iter());
        tag.zeroize();
        Ok(equal)
    }
}

impl<E: BlockEngine> Drop for Cmac<E> {
    fn drop(&mut self) {
        self.l.zeroize();
        self.state.zeroize();
        self.pending.zeroize();
    }
}

/// The RFC 4493 subkey step: shift left by one bit, then add `const_Rb` when
/// the shifted-out bit was set.
fn double(block: &Block) -> Block {
    let mut out = [0; BLOCK];
    let mut carry = 0;
    for (target, source) in out.iter_mut().zip(block).rev() {
        *target = (source << 1) | carry;
        carry = source >> 7;
    }
    if let Some(last) = out.last_mut() {
        // Branch-free: `carry` is 0 or 1.
        *last ^= RB & 0_u8.wrapping_sub(carry);
    }
    out
}

fn xor_into(target: &mut Block, source: &Block) {
    for (out, byte) in target.iter_mut().zip(source) {
        *out ^= byte;
    }
}

fn erase(mut block: Block) {
    block.zeroize();
}

#[cfg(test)]
mod tests;
