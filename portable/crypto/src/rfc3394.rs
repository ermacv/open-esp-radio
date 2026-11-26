//! RFC 3394 AES key wrap: the one copy of the algorithm, over a synchronous
//! block engine.
//!
//! [`unwrap`] is the index-based unwrap (RFC 3394 §2.2.2, second form) and
//! [`wrap`] the index-based wrap (§2.2.1), each to completion on the
//! caller's engine, under the caller's key, in place in the caller's buffer:
//! all three are lent for the call and are the caller's again on return.
//! The buffer has RFC 3394's own layout in both directions: the integrity
//! register's semiblock, then the registers, which hold the key data.
//! Nothing moves: the unwrap answers the plaintext as the registers' slice,
//! and the wrap takes its plaintext after a semiblock of room. The buffer is
//! split once, at admission; a length refusal loads no key and touches no
//! byte. After admission the key is in the engine from its load to the keyed
//! view's drop, before the call returns, and a refusal clears the whole
//! buffer. Received data reaches the unwrap; nothing here panics on it.

use crate::{
    Aes128Key,
    block::{Aes128Engine, BlockEngine},
};
use zeroize::Zeroize;

/// Why an RFC 3394 unwrap refused its input or its engine stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyUnwrapError<S> {
    /// The wrapped data is not 64-bit blocks with at least two blocks of
    /// key data (RFC 3394 §2).
    InvalidLength,
    /// The initial value did not verify (RFC 3394 §2.2.3).
    IntegrityCheckFailed,
    /// The engine stopped; the plaintext and intermediate state are erased.
    Engine(S),
}

/// The default initial value (RFC 3394 §2.2.3.1).
const IV: [u8; 8] = [0xa6; 8];

/// Six passes over the key data (RFC 3394 §2.2.1, §2.2.2).
const LAST_ROUND: u8 = 5;

/// One semiblock.
const SEMIBLOCK: usize = 8;

/// The admitted semiblocks of a buffer: the integrity register `A` and the
/// registers `R[1..=n]`, `n >= 2`, a whole number of semiblocks (RFC 3394
/// §2). The one length check of either direction makes it, and the passes
/// walk its registers.
struct Semiblocks<'a> {
    first: &'a mut [u8; SEMIBLOCK],
    registers: &'a mut [u8],
}

impl Semiblocks<'_> {
    /// The registers `R[1..=n]`, one semiblock at a time.
    fn registers(&mut self) -> Registers<'_> {
        Registers(&mut *self.registers)
    }
}

/// `bytes` as semiblocks, if RFC 3394 admits their length.
fn semiblocks(bytes: &mut [u8]) -> Option<Semiblocks<'_>> {
    let (first, registers) = bytes.split_first_chunk_mut::<SEMIBLOCK>()?;
    if registers.len() < 2 * SEMIBLOCK || !registers.len().is_multiple_of(SEMIBLOCK) {
        return None;
    }
    Some(Semiblocks { first, registers })
}

/// The registers of admitted semiblocks, from either end. Admission left no
/// partial semiblock, so the walk ends exactly at the last register.
struct Registers<'a>(&'a mut [u8]);

impl<'a> Iterator for Registers<'a> {
    type Item = &'a mut [u8; SEMIBLOCK];

    fn next(&mut self) -> Option<Self::Item> {
        let (register, rest) = core::mem::take(&mut self.0).split_first_chunk_mut::<SEMIBLOCK>()?;
        self.0 = rest;
        Some(register)
    }
}

impl DoubleEndedIterator for Registers<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let (rest, register) = core::mem::take(&mut self.0).split_last_chunk_mut::<SEMIBLOCK>()?;
        self.0 = rest;
        Some(register)
    }
}

/// `6n`, the `t` of the wrap's last step and of the unwrap's first.
fn last_step(semiblocks: &mut Semiblocks<'_>) -> u64 {
    // Six steps a register of a buffer that fits memory: far below `u64::MAX`.
    semiblocks.registers().fold(0_u64, |steps, _| {
        steps.wrapping_add(u64::from(LAST_ROUND) + 1)
    })
}

/// Unwrap `wrapped` (`n + 1` semiblocks with `n >= 2`, the whole buffer)
/// in place under `kek` on `engine`: the `Ok` is the plaintext key data, the
/// `n` semiblocks after the first, which holds the verified initial value.
/// A length refusal decrypts nothing and leaves the buffer as it was; an
/// integrity failure or the engine's stop clears the whole buffer.
pub fn unwrap<'b, E: Aes128Engine>(
    engine: &mut E,
    kek: &Aes128Key,
    wrapped: &'b mut [u8],
) -> Result<&'b mut [u8], KeyUnwrapError<E::Stop>> {
    let Some(mut semiblocks) = semiblocks(wrapped) else {
        return Err(KeyUnwrapError::InvalidLength);
    };
    match unwrap_semiblocks(engine, kek, &mut semiblocks) {
        Ok(()) => Ok(semiblocks.registers),
        Err(error) => {
            semiblocks.first.zeroize();
            semiblocks.registers.zeroize();
            Err(error)
        }
    }
}

/// The passes over admitted semiblocks, then the integrity check.
fn unwrap_semiblocks<E: Aes128Engine>(
    engine: &mut E,
    kek: &Aes128Key,
    semiblocks: &mut Semiblocks<'_>,
) -> Result<(), KeyUnwrapError<E::Stop>> {
    let mut cipher = engine.load(kek).map_err(KeyUnwrapError::Engine)?;
    let mut block = [0; 16];
    let blocks = unwrap_blocks(&mut cipher, semiblocks, &mut block);
    drop(cipher);
    block.zeroize();
    blocks.map_err(KeyUnwrapError::Engine)?;
    if crate::compare::constant_time_eq(semiblocks.first.iter(), IV.iter()) {
        Ok(())
    } else {
        Err(KeyUnwrapError::IntegrityCheckFailed)
    }
}

/// The six passes: for `j` from 5 down to 0 and `i` from `n` down to 1,
/// `B = AES-1(K, (A ^ t) | R[i])` with `t = n * j + i`, then `A = MSB(64, B)`
/// and `R[i] = LSB(64, B)`. `A` is the buffer's first semiblock.
fn unwrap_blocks<K: BlockEngine>(
    cipher: &mut K,
    semiblocks: &mut Semiblocks<'_>,
    block: &mut [u8; 16],
) -> Result<(), K::Stop> {
    let mut step = last_step(semiblocks);
    for _ in 0..=LAST_ROUND {
        for register in Registers(&mut *semiblocks.registers).rev() {
            for ((out, value), count) in block
                .iter_mut()
                .zip(semiblocks.first.iter())
                .zip(step.to_be_bytes())
            {
                *out = value ^ count;
            }
            for (out, byte) in block.iter_mut().skip(SEMIBLOCK).zip(register.iter()) {
                *out = *byte;
            }
            cipher.decrypt(block)?;
            for (out, byte) in semiblocks
                .first
                .iter_mut()
                .chain(register.iter_mut())
                .zip(block.iter())
            {
                *out = *byte;
            }
            step = step.wrapping_sub(1);
        }
    }
    Ok(())
}

/// Why [`wrap`] refused its plaintext.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyWrapError<S> {
    /// Not 64-bit blocks, or fewer than two (RFC 3394 §2).
    InvalidLength,
    /// The engine stopped; the buffer and intermediate state are erased.
    Engine(S),
}

/// Wrap the plaintext key data that follows a semiblock of room in `buffer`
/// (`n >= 2` semiblocks after the first, RFC 3394 §2.2.1, index-based form)
/// in place under `kek` on `engine`: on `Ok` the whole buffer is the
/// ciphertext, its first semiblock the integrity register. A length refusal
/// leaves the buffer as it was; the engine's stop clears the whole buffer.
pub fn wrap<E: Aes128Engine>(
    engine: &mut E,
    kek: &Aes128Key,
    buffer: &mut [u8],
) -> Result<(), KeyWrapError<E::Stop>> {
    let Some(mut semiblocks) = semiblocks(buffer) else {
        return Err(KeyWrapError::InvalidLength);
    };
    *semiblocks.first = IV;
    let outcome = wrap_semiblocks(engine, kek, &mut semiblocks);
    if outcome.is_err() {
        semiblocks.first.zeroize();
        semiblocks.registers.zeroize();
    }
    outcome
}

/// The passes over admitted semiblocks, the integrity register set.
fn wrap_semiblocks<E: Aes128Engine>(
    engine: &mut E,
    kek: &Aes128Key,
    semiblocks: &mut Semiblocks<'_>,
) -> Result<(), KeyWrapError<E::Stop>> {
    let mut cipher = engine.load(kek).map_err(KeyWrapError::Engine)?;
    let mut block = [0; 16];
    let blocks = wrap_blocks(&mut cipher, semiblocks, &mut block);
    drop(cipher);
    block.zeroize();
    blocks.map_err(KeyWrapError::Engine)
}

/// The six passes: for `j` from 0 to 5 and `i` from 1 to `n`,
/// `B = AES(K, A | R[i])`, then `A = MSB(64, B) ^ t` with `t = n * j + i`
/// and `R[i] = LSB(64, B)`. `A` is the buffer's first semiblock.
fn wrap_blocks<K: BlockEngine>(
    cipher: &mut K,
    semiblocks: &mut Semiblocks<'_>,
    block: &mut [u8; 16],
) -> Result<(), K::Stop> {
    let mut step: u64 = 0;
    for _ in 0..=LAST_ROUND {
        for register in Registers(&mut *semiblocks.registers) {
            step = step.wrapping_add(1);
            for (out, byte) in block
                .iter_mut()
                .zip(semiblocks.first.iter().chain(register.iter()))
            {
                *out = *byte;
            }
            cipher.encrypt(block)?;
            for ((out, value), count) in semiblocks
                .first
                .iter_mut()
                .zip(block.iter())
                .zip(step.to_be_bytes())
            {
                *out = value ^ count;
            }
            for (out, byte) in register.iter_mut().zip(block.iter().skip(SEMIBLOCK)) {
                *out = *byte;
            }
        }
    }
    Ok(())
}
