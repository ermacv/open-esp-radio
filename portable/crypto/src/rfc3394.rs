//! RFC 3394 AES key wrap: the one copy of the algorithm, over a synchronous
//! block engine.
//!
//! [`unwrap`] is the index-based unwrap (RFC 3394 §2.2.2, second form) and
//! [`wrap`] the index-based wrap (§2.2.1), each to completion, in place in
//! the caller's buffer. Received data reaches the unwrap; nothing here panics
//! on it.

use crate::{KeyData, KeyUnwrapError, KeyUnwrapResponse, KeyWrapResponse, block::BlockEngine};
use zeroize::Zeroize;

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

/// Unwrap `wrapped` (`n + 1` semiblocks with `n >= 2`) in place, decrypting
/// each block over the engine `load` returns. A length refusal returns before
/// `load` runs; a refused key or block ends the operation with
/// [`KeyUnwrapError::Engine`]. The engine is dropped after the last block, the
/// block is erased on every exit, and a refusal clears the buffer.
pub fn unwrap<K: BlockEngine, const N: usize>(
    wrapped: KeyData<N>,
    load: impl FnOnce() -> Result<K, K::Stop>,
) -> KeyUnwrapResponse<N, K::Stop> {
    let mut data = wrapped;
    match unwrap_in_place(data.held_mut(), load) {
        Ok(()) => {
            // The shift left the last semiblock clear.
            data.len = data.len.saturating_sub(SEMIBLOCK);
            KeyUnwrapResponse {
                data,
                outcome: Ok(()),
            }
        }
        Err(error) => refuse(data, error),
    }
}

fn unwrap_in_place<K: BlockEngine>(
    bytes: &mut [u8],
    load: impl FnOnce() -> Result<K, K::Stop>,
) -> Result<(), KeyUnwrapError<K::Stop>> {
    let Some(mut semiblocks) = semiblocks(bytes) else {
        return Err(KeyUnwrapError::InvalidLength);
    };
    let mut cipher = load().map_err(KeyUnwrapError::Engine)?;
    let mut block = [0; 16];
    let blocks = unwrap_blocks(&mut cipher, &mut semiblocks, &mut block);
    drop(cipher);
    block.zeroize();
    blocks.map_err(KeyUnwrapError::Engine)?;
    if !crate::compare::constant_time_eq(semiblocks.first.iter(), IV.iter()) {
        return Err(KeyUnwrapError::IntegrityCheckFailed);
    }
    shift_plaintext(semiblocks);
    Ok(())
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

/// Move the plaintext `R[1..=n]` one semiblock towards the start, over the
/// verified integrity register, leaving the last semiblock clear.
fn shift_plaintext(mut semiblocks: Semiblocks<'_>) {
    let mut carry = [0; SEMIBLOCK];
    for register in semiblocks.registers().rev() {
        core::mem::swap(register, &mut carry);
    }
    *semiblocks.first = carry;
    carry.zeroize();
}

/// The refusal: the buffer cleared, with `error`.
fn refuse<const N: usize, S>(
    mut data: KeyData<N>,
    error: KeyUnwrapError<S>,
) -> KeyUnwrapResponse<N, S> {
    data.zeroize();
    KeyUnwrapResponse {
        data,
        outcome: Err(error),
    }
}

/// Why [`wrap`] refused its plaintext.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyWrapError<S> {
    /// Not 64-bit blocks, or fewer than two (RFC 3394 §2).
    InvalidLength,
    /// The ciphertext, one semiblock longer, does not fit `N`.
    CapacityExceeded,
    /// The engine stopped; the buffer and intermediate state are erased.
    Engine(S),
}

/// Wrap `plaintext` (`n >= 2` semiblocks, RFC 3394 §2.2.1, index-based
/// form) in place in its buffer of capacity `N`, encrypting each block over
/// the engine `load` returns. The buffer first grows by the integrity
/// register's semiblock, so a plaintext that does not fit with it is refused
/// as [`KeyWrapError::CapacityExceeded`] before its length is examined. A
/// refusal returns before `load` runs; a refused key or block ends the
/// operation with [`KeyWrapError::Engine`]. The engine is dropped after the
/// last block, the block is erased on every exit, and a refusal clears the
/// buffer.
pub fn wrap<K: BlockEngine, const N: usize>(
    plaintext: KeyData<N>,
    load: impl FnOnce() -> Result<K, K::Stop>,
) -> KeyWrapResponse<N, K::Stop> {
    let mut data = plaintext;
    // The bytes after the held ones are zero: the grown semiblock is.
    let Some(len) = data.len.checked_add(SEMIBLOCK).filter(|len| *len <= N) else {
        return refuse_wrap(data, KeyWrapError::CapacityExceeded);
    };
    data.len = len;
    match wrap_in_place(data.held_mut(), load) {
        Ok(()) => KeyWrapResponse {
            data,
            outcome: Ok(()),
        },
        Err(error) => refuse_wrap(data, error),
    }
}

fn wrap_in_place<K: BlockEngine>(
    bytes: &mut [u8],
    load: impl FnOnce() -> Result<K, K::Stop>,
) -> Result<(), KeyWrapError<K::Stop>> {
    let Some(mut semiblocks) = semiblocks(bytes) else {
        return Err(KeyWrapError::InvalidLength);
    };
    // Make room for the integrity register: `R[1..=n]` one semiblock on.
    let mut carry = IV;
    core::mem::swap(semiblocks.first, &mut carry);
    for register in semiblocks.registers() {
        core::mem::swap(register, &mut carry);
    }
    // `carry` holds the grown semiblock's zeros.
    carry.zeroize();
    let mut cipher = load().map_err(KeyWrapError::Engine)?;
    let mut block = [0; 16];
    let blocks = wrap_blocks(&mut cipher, &mut semiblocks, &mut block);
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

/// The wrap's refusal: the buffer cleared, with `error`.
fn refuse_wrap<const N: usize, S>(
    mut data: KeyData<N>,
    error: KeyWrapError<S>,
) -> KeyWrapResponse<N, S> {
    data.zeroize();
    KeyWrapResponse {
        data,
        outcome: Err(error),
    }
}
