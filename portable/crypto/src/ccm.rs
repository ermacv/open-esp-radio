//! CCM (RFC 3610) with a thirteen-byte nonce over a keyed block engine.
//!
//! A thirteen-byte nonce leaves a two-byte length field (`L = 2`), so the
//! body is at most 65535 bytes. The MIC length `M` is a const parameter,
//! checked at compile time against RFC 3610's even values 4 to 16. The
//! associated data is encoded with its two-byte length, so it is shorter than
//! `0xff00` bytes. [`seal`] authenticates the plaintext (CBC-MAC over `B_0`,
//! the encoded associated data and the body), then encrypts the body in place
//! with the counter blocks `A_1..` and masks the MIC with `A_0`. [`open`]
//! decrypts first and compares the MIC in constant time; a MIC that does not
//! verify, or a refused block, erases the body. A block refused while [`seal`]
//! encrypts erases the partly encrypted body. Intermediate blocks are erased
//! on every exit.

use crate::block::{Block, BlockEngine, BlockError};
use zeroize::Zeroize;

const BLOCK: usize = 16;

/// The nonce length that leaves a two-byte length field.
pub const NONCE_LEN: usize = 13;

/// `L - 1` for the two-byte length field, the low bits of every flags byte.
const LENGTH_FIELD: u8 = 1;

/// Associated data from this length on would need a six-byte length encoding.
const AAD_LIMIT: usize = 0xff00;

/// Why a CCM operation produced no result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmError {
    /// The associated data is `0xff00` bytes or longer.
    AadTooLong,
    /// The body exceeds the two-byte length field.
    BodyTooLong,
    /// The MIC does not verify; the body is erased.
    InvalidMic,
    /// The engine refused a block; the body is erased.
    Engine(BlockError),
}

/// Encrypt `body` in place and return its MIC.
pub fn seal<E: BlockEngine, const M: usize>(
    engine: &mut E,
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    body: &mut [u8],
) -> Result<[u8; M], CcmError> {
    let length = admit::<M>(aad, body)?;
    let mut tag = cbc_mac::<E, M>(engine, nonce, aad, body, length)?;
    let result = counter_pass(engine, nonce, body).and_then(|()| mask(engine, nonce, &mut tag));
    match result {
        Ok(mic) => {
            tag.zeroize();
            Ok(mic)
        }
        Err(error) => {
            tag.zeroize();
            body.zeroize();
            Err(CcmError::Engine(error))
        }
    }
}

/// Decrypt `body` in place and verify `mic`. On any error after admission the
/// body is erased.
pub fn open<E: BlockEngine, const M: usize>(
    engine: &mut E,
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    body: &mut [u8],
    mic: &[u8; M],
) -> Result<(), CcmError> {
    let length = admit::<M>(aad, body)?;
    if let Err(error) = counter_pass(engine, nonce, body) {
        body.zeroize();
        return Err(CcmError::Engine(error));
    }
    let mut tag = match cbc_mac::<E, M>(engine, nonce, aad, body, length) {
        Ok(tag) => tag,
        Err(error) => {
            body.zeroize();
            return Err(error);
        }
    };
    let expected = match mask::<E, M>(engine, nonce, &mut tag) {
        Ok(expected) => expected,
        Err(error) => {
            tag.zeroize();
            body.zeroize();
            return Err(CcmError::Engine(error));
        }
    };
    tag.zeroize();
    if crate::compare::constant_time_eq(expected.iter(), mic.iter()) {
        Ok(())
    } else {
        body.zeroize();
        Err(CcmError::InvalidMic)
    }
}

/// Check the MIC length at compile time and the lengths of this call.
fn admit<const M: usize>(aad: &[u8], body: &[u8]) -> Result<u16, CcmError> {
    const {
        if M < 4 || M > BLOCK || !M.is_multiple_of(2) {
            panic!("a CCM MIC is 4, 6, 8, 10, 12, 14 or 16 bytes");
        }
    }
    if aad.len() >= AAD_LIMIT {
        return Err(CcmError::AadTooLong);
    }
    u16::try_from(body.len()).map_err(|_| CcmError::BodyTooLong)
}

/// The unmasked CBC-MAC `T` over `B_0`, the encoded associated data and the
/// plaintext `body`.
fn cbc_mac<E: BlockEngine, const M: usize>(
    engine: &mut E,
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    body: &[u8],
    length: u16,
) -> Result<Block, CcmError> {
    // `M` is checked even and at most 16 by `admit`, so this fits three bits.
    let mic_field = u8::try_from(M.saturating_sub(2) / 2).unwrap_or(0);
    let adata = if aad.is_empty() { 0 } else { 0x40 };
    let flags = adata | (mic_field << 3) | LENGTH_FIELD;
    let mut mac = CbcMac {
        state: block(flags, nonce, length),
        pending: [0; BLOCK],
        filled: 0,
    };
    let result = (|| {
        engine.encrypt(&mut mac.state)?;
        if !aad.is_empty() {
            // `admit` bounds the length below 0xff00.
            let aad_length = u16::try_from(aad.len()).unwrap_or(0);
            mac.feed(engine, &aad_length.to_be_bytes())?;
            mac.feed(engine, aad)?;
            mac.pad(engine)?;
        }
        mac.feed(engine, body)?;
        mac.pad(engine)
    })();
    match result {
        Ok(()) => Ok(mac.state),
        Err(error) => Err(CcmError::Engine(error)),
    }
}

/// Encrypt (or decrypt) `body` in place with the counter blocks `A_1..`.
fn counter_pass<E: BlockEngine>(
    engine: &mut E,
    nonce: &[u8; NONCE_LEN],
    body: &mut [u8],
) -> Result<(), BlockError> {
    let mut stream: Block = [0; BLOCK];
    let mut used = BLOCK;
    let mut counter: u16 = 0;
    let mut result = Ok(());
    for byte in body.iter_mut() {
        if used >= BLOCK {
            // The body is at most 65535 bytes, so at most 4096 counter blocks.
            counter = counter.wrapping_add(1);
            stream = block(LENGTH_FIELD, nonce, counter);
            if let Err(error) = engine.encrypt(&mut stream) {
                result = Err(error);
                break;
            }
            used = 0;
        }
        *byte ^= stream.get(used).copied().unwrap_or(0);
        used = used.saturating_add(1);
    }
    stream.zeroize();
    result
}

/// The MIC: the first `M` bytes of `T` masked with `E(K, A_0)`.
fn mask<E: BlockEngine, const M: usize>(
    engine: &mut E,
    nonce: &[u8; NONCE_LEN],
    tag: &mut Block,
) -> Result<[u8; M], BlockError> {
    let mut s0 = block(LENGTH_FIELD, nonce, 0);
    engine.encrypt(&mut s0)?;
    let mut mic = [0; M];
    for ((out, value), mask) in mic.iter_mut().zip(tag.iter()).zip(s0) {
        *out = value ^ mask;
    }
    s0.zeroize();
    Ok(mic)
}

/// A CCM block: `flags`, the nonce and a two-byte big-endian `field`.
fn block(flags: u8, nonce: &[u8; NONCE_LEN], field: u16) -> Block {
    let mut out: Block = [0; BLOCK];
    for (slot, byte) in out.iter_mut().zip(
        core::iter::once(&flags)
            .chain(nonce)
            .chain(&field.to_be_bytes()),
    ) {
        *slot = *byte;
    }
    out
}

/// CBC-MAC over bytes fed in pieces, each segment zero-padded to a block.
struct CbcMac {
    state: Block,
    pending: Block,
    filled: usize,
}

impl CbcMac {
    fn feed<E: BlockEngine>(&mut self, engine: &mut E, bytes: &[u8]) -> Result<(), BlockError> {
        for byte in bytes {
            if let Some(slot) = self.pending.get_mut(self.filled) {
                *slot = *byte;
            }
            self.filled = self.filled.saturating_add(1);
            if self.filled >= BLOCK {
                self.absorb(engine)?;
            }
        }
        Ok(())
    }

    /// Absorb a partial block, zero-padded.
    fn pad<E: BlockEngine>(&mut self, engine: &mut E) -> Result<(), BlockError> {
        if self.filled > 0 {
            self.absorb(engine)?;
        }
        Ok(())
    }

    fn absorb<E: BlockEngine>(&mut self, engine: &mut E) -> Result<(), BlockError> {
        for (out, byte) in self.state.iter_mut().zip(&self.pending) {
            *out ^= byte;
        }
        self.pending.zeroize();
        self.filled = 0;
        engine.encrypt(&mut self.state)
    }
}

impl Drop for CbcMac {
    fn drop(&mut self) {
        self.state.zeroize();
        self.pending.zeroize();
    }
}

#[cfg(test)]
mod tests;
