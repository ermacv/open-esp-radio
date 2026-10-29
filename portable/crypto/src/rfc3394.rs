//! RFC 3394 AES key wrap: the one copy of the algorithm, over a synchronous
//! [`BlockCipher`].
//!
//! [`Rfc3394Unwrap`] is resumable: each [`Rfc3394Unwrap::next_block`] issues one
//! block of the index-based unwrap (RFC 3394 §2.2.2, second form), so an
//! engine may yield while a block is in flight and between blocks. [`wrap`] is the index-based wrap (§2.2.1)
//! to completion. Received data reaches the unwrap; nothing here panics on it.

use crate::{
    KeyData, KeyUnwrapError, KeyUnwrapResponse,
    block::{BlockCipher, BlockError, CipherBlock},
};
use zeroize::Zeroize;

/// The default initial value (RFC 3394 §2.2.3.1).
const IV: [u8; 8] = [0xa6; 8];

/// Six passes over the key data (RFC 3394 §2.2.1, §2.2.2).
const LAST_ROUND: u8 = 5;

/// One semiblock.
const SEMIBLOCK: usize = 8;

/// An unwrap in progress: the buffer holds the ciphertext's registers
/// `R[1..=n]` after the first semiblock.
pub struct Rfc3394Unwrap<const N: usize> {
    data: KeyData<N>,
    /// The integrity register `A`.
    accumulator: [u8; 8],
    /// `n`, the number of key-data semiblocks.
    semiblocks: u64,
    /// The pass `j`, from 5 down to 0.
    round: u8,
    /// The register `i` of the next step, from `n` down to 1.
    index: u64,
}

/// The machine after one step.
pub enum Rfc3394Step<const N: usize> {
    /// One owned input block and the unique continuation accepting its result.
    Block {
        input: CipherBlock,
        pending: Rfc3394Pending<N>,
    },
    /// The response, with the plaintext or the refusal.
    Complete(KeyUnwrapResponse<N>),
}

/// The mode state while a block engine owns its input. It accepts one result;
/// dropping it erases the intermediate key data and integrity register.
pub struct Rfc3394Pending<const N: usize>(Rfc3394Unwrap<N>);

impl<const N: usize> Rfc3394Unwrap<N> {
    /// Admit wrapped data: `n + 1` semiblocks with `n >= 2`. A refusal
    /// returns the response at once, without a block operation.
    pub fn start(wrapped: KeyData<N>) -> Result<Self, KeyUnwrapResponse<N>> {
        let len = wrapped.as_bytes().len();
        let Some(accumulator) = wrapped.as_bytes().first_chunk::<SEMIBLOCK>().copied() else {
            return Err(KeyUnwrapResponse::refused(KeyUnwrapError::InvalidLength));
        };
        if !len.is_multiple_of(SEMIBLOCK) || len < 3 * SEMIBLOCK {
            return Err(KeyUnwrapResponse::refused(KeyUnwrapError::InvalidLength));
        }
        let semiblocks = (len / SEMIBLOCK).saturating_sub(1) as u64;
        Ok(Self {
            data: wrapped,
            accumulator,
            semiblocks,
            round: LAST_ROUND,
            index: semiblocks,
        })
    }

    /// Issue `AES-1(K, (A ^ t) | R[i])`, with `t = n * j + i`.
    /// No mode state advances until the returned continuation accepts a result.
    pub fn next_block(mut self) -> Rfc3394Step<N> {
        let Some(register) = register(&mut self.data, self.index) else {
            return Rfc3394Step::Complete(self.refuse(KeyUnwrapError::InvalidLength));
        };
        let Some(counter) = u64::from(self.round)
            .checked_mul(self.semiblocks)
            .and_then(|base| base.checked_add(self.index))
        else {
            return Rfc3394Step::Complete(self.refuse(KeyUnwrapError::InvalidLength));
        };
        let mut high = [0; SEMIBLOCK];
        for ((out, value), count) in high
            .iter_mut()
            .zip(self.accumulator)
            .zip(counter.to_be_bytes())
        {
            *out = value ^ count;
        }
        let input = CipherBlock::new(join(&high, register));
        high.zeroize();
        Rfc3394Step::Block {
            input,
            pending: Rfc3394Pending(self),
        }
    }

    /// Terminate an admitted operation before submitting a block.
    pub fn fail(self, error: BlockError) -> KeyUnwrapResponse<N> {
        self.refuse(KeyUnwrapError::Engine(error))
    }

    /// Check the integrity register and move the plaintext `R[1..=n]` to the
    /// start of the buffer.
    fn finish(mut self) -> KeyUnwrapResponse<N> {
        let mut difference = 0;
        for (actual, expected) in self.accumulator.iter().zip(IV) {
            difference |= actual ^ expected;
        }
        if difference != 0 {
            return self.refuse(KeyUnwrapError::IntegrityCheckFailed);
        }
        let len = self.data.len;
        let plain = len.saturating_sub(SEMIBLOCK);
        let mut target = 0;
        while target < plain {
            let source = target.saturating_add(SEMIBLOCK);
            match (
                self.data.bytes.get(source).copied(),
                self.data.bytes.get_mut(target),
            ) {
                (Some(byte), Some(out)) => *out = byte,
                _ => return self.refuse(KeyUnwrapError::InvalidLength),
            }
            target = target.saturating_add(1);
        }
        for byte in self.data.bytes.iter_mut().take(len).skip(plain) {
            *byte = 0;
        }
        self.data.len = plain;
        self.accumulator.zeroize();
        KeyUnwrapResponse {
            data: core::mem::replace(&mut self.data, KeyData::cleared()),
            outcome: Ok(()),
        }
    }

    fn refuse(mut self, error: KeyUnwrapError) -> KeyUnwrapResponse<N> {
        self.data.zeroize();
        self.accumulator.zeroize();
        KeyUnwrapResponse {
            data: core::mem::replace(&mut self.data, KeyData::cleared()),
            outcome: Err(error),
        }
    }
}

impl<const N: usize> Rfc3394Pending<N> {
    /// Terminate while the block is in flight, erasing all mode state.
    pub fn fail(self, error: BlockError) -> KeyUnwrapResponse<N> {
        self.0.fail(error)
    }

    /// Accept exactly one completed block or a typed engine error.
    pub fn complete(self, result: Result<CipherBlock, BlockError>) -> Rfc3394Step<N> {
        let mut machine = self.0;
        let block = match result {
            Ok(block) => block,
            Err(error) => return Rfc3394Step::Complete(machine.fail(error)),
        };
        let Some(register) = register(&mut machine.data, machine.index) else {
            return Rfc3394Step::Complete(machine.refuse(KeyUnwrapError::InvalidLength));
        };
        (machine.accumulator, *register) = split(block.as_bytes());
        drop(block);
        match (machine.index.checked_sub(1), machine.round.checked_sub(1)) {
            (Some(index @ 1..), _) => machine.index = index,
            (_, Some(round)) => {
                machine.round = round;
                machine.index = machine.semiblocks;
            }
            (_, None) => return Rfc3394Step::Complete(machine.finish()),
        }
        machine.next_block()
    }
}

impl<const N: usize> Drop for Rfc3394Unwrap<N> {
    fn drop(&mut self) {
        self.accumulator.zeroize();
    }
}

/// Why [`wrap`] refused its plaintext.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyWrapError {
    /// Not 64-bit blocks, or fewer than two (RFC 3394 §2).
    InvalidLength,
    /// The ciphertext, one semiblock longer, does not fit `N`.
    CapacityExceeded,
}

/// Wrap `plaintext` (RFC 3394 §2.2.1, index-based form) into a buffer of
/// capacity `N`. Inputs outside the domain fail before a block operation.
pub fn wrap<const N: usize>(
    cipher: &mut impl BlockCipher,
    plaintext: &[u8],
) -> Result<KeyData<N>, KeyWrapError> {
    if plaintext.len() < 2 * SEMIBLOCK || !plaintext.len().is_multiple_of(SEMIBLOCK) {
        return Err(KeyWrapError::InvalidLength);
    }
    let len = plaintext
        .len()
        .checked_add(SEMIBLOCK)
        .ok_or(KeyWrapError::CapacityExceeded)?;
    if len > N {
        return Err(KeyWrapError::CapacityExceeded);
    }
    let mut output = KeyData::<N>::cleared();
    output.len = len;
    for (out, byte) in output.bytes.iter_mut().skip(SEMIBLOCK).zip(plaintext) {
        *out = *byte;
    }
    let semiblocks = (plaintext.len() / SEMIBLOCK) as u64;
    let mut accumulator = IV;
    let mut block = [0; 16];
    for round in 0..=u64::from(LAST_ROUND) {
        for index in 1..=semiblocks {
            let register = register(&mut output, index).ok_or(KeyWrapError::CapacityExceeded)?;
            block = join(&accumulator, register);
            cipher.encrypt_block(&mut block);
            let counter = round
                .checked_mul(semiblocks)
                .and_then(|base| base.checked_add(index))
                .ok_or(KeyWrapError::CapacityExceeded)?;
            let mut high;
            (high, *register) = split(&block);
            for ((out, value), count) in accumulator.iter_mut().zip(high).zip(counter.to_be_bytes())
            {
                *out = value ^ count;
            }
            high.zeroize();
        }
    }
    block.zeroize();
    if let Some(first) = output.bytes.first_chunk_mut::<SEMIBLOCK>() {
        *first = accumulator;
    }
    accumulator.zeroize();
    Ok(output)
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
