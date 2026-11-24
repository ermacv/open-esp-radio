//! RFC 3394 AES key wrap: the one copy of the algorithm, over a synchronous
//! block engine.
//!
//! [`unwrap`] is the index-based unwrap (RFC 3394 §2.2.2, second form) and
//! [`wrap`] the index-based wrap (§2.2.1), each to completion, in place in
//! the caller's buffer. Received data reaches the unwrap; nothing here panics
//! on it.

use crate::{
    KeyData, KeyUnwrapError, KeyUnwrapResponse, KeyWrapResponse,
    block::{BlockEngine, BlockError},
};
use zeroize::Zeroize;

/// The default initial value (RFC 3394 §2.2.3.1).
const IV: [u8; 8] = [0xa6; 8];

/// Six passes over the key data (RFC 3394 §2.2.1, §2.2.2).
const LAST_ROUND: u8 = 5;

/// One semiblock.
const SEMIBLOCK: usize = 8;

/// Unwrap `wrapped` (`n + 1` semiblocks with `n >= 2`) in place, decrypting
/// each block over the engine `load` returns. A length refusal returns before
/// `load` runs; a refused key or block ends the operation with
/// [`KeyUnwrapError::Engine`]. The engine is dropped after the last block, and
/// the integrity register and the block are erased on every exit; a refusal
/// clears the buffer.
pub fn unwrap<K: BlockEngine, const N: usize>(
    wrapped: KeyData<N>,
    load: impl FnOnce() -> Result<K, BlockError>,
) -> KeyUnwrapResponse<N> {
    let mut data = wrapped;
    let Some(semiblocks) = admitted_semiblocks(data.as_bytes().len()) else {
        return refuse(data, KeyUnwrapError::InvalidLength);
    };
    let mut cipher = match load() {
        Ok(cipher) => cipher,
        Err(error) => return refuse(data, KeyUnwrapError::Engine(error)),
    };
    let mut accumulator = [0; SEMIBLOCK];
    let mut block = [0; 16];
    let blocks = unwrap_blocks(
        &mut cipher,
        &mut data,
        semiblocks,
        &mut accumulator,
        &mut block,
    );
    drop(cipher);
    block.zeroize();
    let outcome = blocks
        .and_then(|()| verify(&accumulator))
        .and_then(|()| shift_plaintext(&mut data));
    accumulator.zeroize();
    match outcome {
        Ok(()) => KeyUnwrapResponse {
            data,
            outcome: Ok(()),
        },
        Err(error) => refuse(data, error),
    }
}

/// `n` for `len` bytes of wrapped data, if RFC 3394 admits them.
fn admitted_semiblocks(len: usize) -> Option<u64> {
    if !len.is_multiple_of(SEMIBLOCK) || len < 3 * SEMIBLOCK {
        return None;
    }
    u64::try_from(len / SEMIBLOCK).ok()?.checked_sub(1)
}

/// The six passes: for `j` from 5 down to 0 and `i` from `n` down to 1,
/// `B = AES-1(K, (A ^ t) | R[i])` with `t = n * j + i`, then `A = MSB(64, B)`
/// and `R[i] = LSB(64, B)`.
fn unwrap_blocks<K: BlockEngine, const N: usize>(
    cipher: &mut K,
    data: &mut KeyData<N>,
    semiblocks: u64,
    accumulator: &mut [u8; SEMIBLOCK],
    block: &mut [u8; 16],
) -> Result<(), KeyUnwrapError> {
    *accumulator = *data
        .as_bytes()
        .first_chunk::<SEMIBLOCK>()
        .ok_or(KeyUnwrapError::InvalidLength)?;
    for round in (0..=u64::from(LAST_ROUND)).rev() {
        for index in (1..=semiblocks).rev() {
            let counter = round
                .checked_mul(semiblocks)
                .and_then(|base| base.checked_add(index))
                .ok_or(KeyUnwrapError::InvalidLength)?;
            let register = register(data, index).ok_or(KeyUnwrapError::InvalidLength)?;
            for ((out, value), count) in block
                .iter_mut()
                .zip(accumulator.iter())
                .zip(counter.to_be_bytes())
            {
                *out = value ^ count;
            }
            for (out, byte) in block.iter_mut().skip(SEMIBLOCK).zip(register.iter()) {
                *out = *byte;
            }
            cipher.decrypt(block).map_err(KeyUnwrapError::Engine)?;
            for (out, byte) in accumulator
                .iter_mut()
                .chain(register.iter_mut())
                .zip(block.iter())
            {
                *out = *byte;
            }
        }
    }
    Ok(())
}

/// Compare the integrity register with the initial value in constant time.
fn verify(accumulator: &[u8; SEMIBLOCK]) -> Result<(), KeyUnwrapError> {
    let mut difference = 0;
    for (actual, expected) in accumulator.iter().zip(IV) {
        difference |= actual ^ expected;
    }
    if difference != 0 {
        return Err(KeyUnwrapError::IntegrityCheckFailed);
    }
    Ok(())
}

/// Move the plaintext `R[1..=n]` to the start of the buffer and clear the
/// semiblock it leaves.
fn shift_plaintext<const N: usize>(data: &mut KeyData<N>) -> Result<(), KeyUnwrapError> {
    let len = data.len;
    let plain = len.saturating_sub(SEMIBLOCK);
    let mut target = 0;
    while target < plain {
        let source = target.saturating_add(SEMIBLOCK);
        match (data.bytes.get(source).copied(), data.bytes.get_mut(target)) {
            (Some(byte), Some(out)) => *out = byte,
            _ => return Err(KeyUnwrapError::InvalidLength),
        }
        target = target.saturating_add(1);
    }
    for byte in data.bytes.iter_mut().take(len).skip(plain) {
        *byte = 0;
    }
    data.len = plain;
    Ok(())
}

/// The refusal: the buffer cleared, with `error`.
fn refuse<const N: usize>(mut data: KeyData<N>, error: KeyUnwrapError) -> KeyUnwrapResponse<N> {
    data.zeroize();
    KeyUnwrapResponse {
        data,
        outcome: Err(error),
    }
}

/// Why [`wrap`] refused its plaintext.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyWrapError {
    /// Not 64-bit blocks, or fewer than two (RFC 3394 §2).
    InvalidLength,
    /// The ciphertext, one semiblock longer, does not fit `N`.
    CapacityExceeded,
    /// A block engine failed; the buffer and intermediate state are erased.
    Engine(BlockError),
}

/// Wrap `plaintext` (`n >= 2` semiblocks, RFC 3394 §2.2.1, index-based
/// form) in place in its buffer of capacity `N`, encrypting each block over
/// the engine `load` returns. A length refusal returns before `load` runs; a
/// refused key or block ends the operation with [`KeyWrapError::Engine`].
/// The engine is dropped after the last block, the integrity register and the
/// block are erased on every exit, and a refusal clears the buffer.
pub fn wrap<K: BlockEngine, const N: usize>(
    plaintext: KeyData<N>,
    load: impl FnOnce() -> Result<K, BlockError>,
) -> KeyWrapResponse<N> {
    let mut data = plaintext;
    let plain = data.len;
    if plain < 2 * SEMIBLOCK || !plain.is_multiple_of(SEMIBLOCK) {
        return refuse_wrap(data, KeyWrapError::InvalidLength);
    }
    let Some(len) = plain.checked_add(SEMIBLOCK).filter(|len| *len <= N) else {
        return refuse_wrap(data, KeyWrapError::CapacityExceeded);
    };
    let Ok(semiblocks) = u64::try_from(plain / SEMIBLOCK) else {
        return refuse_wrap(data, KeyWrapError::CapacityExceeded);
    };
    // Make room for the integrity register: `R[1..=n]` one semiblock on.
    let mut source = plain;
    while source > 0 {
        source = source.saturating_sub(1);
        let target = source.saturating_add(SEMIBLOCK);
        match (data.bytes.get(source).copied(), data.bytes.get_mut(target)) {
            (Some(byte), Some(out)) => *out = byte,
            _ => return refuse_wrap(data, KeyWrapError::CapacityExceeded),
        }
    }
    data.len = len;
    let mut cipher = match load() {
        Ok(cipher) => cipher,
        Err(error) => return refuse_wrap(data, KeyWrapError::Engine(error)),
    };
    let mut accumulator = IV;
    let mut block = [0; 16];
    let blocks = wrap_blocks(
        &mut cipher,
        &mut data,
        semiblocks,
        &mut accumulator,
        &mut block,
    );
    drop(cipher);
    block.zeroize();
    if blocks.is_ok()
        && let Some(first) = data.bytes.first_chunk_mut::<SEMIBLOCK>()
    {
        *first = accumulator;
    }
    accumulator.zeroize();
    match blocks {
        Ok(()) => KeyWrapResponse {
            data,
            outcome: Ok(()),
        },
        Err(error) => refuse_wrap(data, error),
    }
}

/// The six passes: for `j` from 0 to 5 and `i` from 1 to `n`,
/// `B = AES(K, A | R[i])`, then `A = MSB(64, B) ^ t` with `t = n * j + i`
/// and `R[i] = LSB(64, B)`.
fn wrap_blocks<K: BlockEngine, const N: usize>(
    cipher: &mut K,
    data: &mut KeyData<N>,
    semiblocks: u64,
    accumulator: &mut [u8; SEMIBLOCK],
    block: &mut [u8; 16],
) -> Result<(), KeyWrapError> {
    for round in 0..=u64::from(LAST_ROUND) {
        for index in 1..=semiblocks {
            let register = register(data, index).ok_or(KeyWrapError::CapacityExceeded)?;
            *block = join(accumulator, register);
            cipher.encrypt(block).map_err(KeyWrapError::Engine)?;
            let counter = round
                .checked_mul(semiblocks)
                .and_then(|base| base.checked_add(index))
                .ok_or(KeyWrapError::CapacityExceeded)?;
            let mut high;
            (high, *register) = split(block);
            for ((out, value), count) in accumulator.iter_mut().zip(high).zip(counter.to_be_bytes())
            {
                *out = value ^ count;
            }
            high.zeroize();
        }
    }
    Ok(())
}

/// The wrap's refusal: the buffer cleared, with `error`.
fn refuse_wrap<const N: usize>(mut data: KeyData<N>, error: KeyWrapError) -> KeyWrapResponse<N> {
    data.zeroize();
    KeyWrapResponse {
        data,
        outcome: Err(error),
    }
}

/// The register `R[index]`, `index >= 1`, after the first semiblock.
fn register<const N: usize>(data: &mut KeyData<N>, index: u64) -> Option<&mut [u8; SEMIBLOCK]> {
    let start = usize::try_from(index).ok()?.checked_mul(SEMIBLOCK)?;
    let end = start.checked_add(SEMIBLOCK)?;
    if end > data.len {
        return None;
    }
    data.bytes
        .get_mut(start..end)?
        .first_chunk_mut::<SEMIBLOCK>()
}

/// The block `high | low`.
fn join(high: &[u8; SEMIBLOCK], low: &[u8; SEMIBLOCK]) -> [u8; 16] {
    let mut block = [0; 16];
    for (out, byte) in block.iter_mut().zip(high.iter().chain(low)) {
        *out = *byte;
    }
    block
}

/// `(MSB(64, block), LSB(64, block))`.
fn split(block: &[u8; 16]) -> ([u8; SEMIBLOCK], [u8; SEMIBLOCK]) {
    let (mut high, mut low) = ([0; SEMIBLOCK], [0; SEMIBLOCK]);
    for (out, byte) in high.iter_mut().chain(low.iter_mut()).zip(block) {
        *out = *byte;
    }
    (high, low)
}
