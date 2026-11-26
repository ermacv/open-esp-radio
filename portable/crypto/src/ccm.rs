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

use crate::block::{Block, BlockEngine};
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
pub enum CcmError<S> {
    /// The associated data is `0xff00` bytes or longer.
    AadTooLong,
    /// The body exceeds the two-byte length field.
    BodyTooLong,
    /// The MIC does not verify; the body is erased.
    InvalidMic,
    /// The engine stopped; the body is erased.
    Engine(S),
}

/// Encrypt `body` in place and return its MIC.
pub fn seal<E: BlockEngine, const M: usize>(
    engine: &mut E,
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    body: &mut [u8],
) -> Result<[u8; M], CcmError<E::Stop>> {
    let lengths = admit::<M, E::Stop>(aad, body)?;
    let mut tag = [0; BLOCK];
    // A refusal before the body is encrypted leaves it as it was.
    if let Err(error) = cbc_mac::<E, M>(engine, nonce, aad, body, lengths, &mut tag) {
        tag.zeroize();
        return Err(CcmError::Engine(error));
    }
    let result = counter_pass(engine, nonce, body).and_then(|()| mask(engine, nonce, &tag));
    tag.zeroize();
    result.map_err(|error| {
        body.zeroize();
        CcmError::Engine(error)
    })
}

/// Decrypt `body` in place and verify `mic`. On any error after admission the
/// body is erased.
pub fn open<E: BlockEngine, const M: usize>(
    engine: &mut E,
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    body: &mut [u8],
    mic: &[u8; M],
) -> Result<(), CcmError<E::Stop>> {
    let lengths = admit::<M, E::Stop>(aad, body)?;
    let mut tag = [0; BLOCK];
    let expected = counter_pass(engine, nonce, body)
        .and_then(|()| cbc_mac::<E, M>(engine, nonce, aad, body, lengths, &mut tag))
        .and_then(|()| mask::<E, M>(engine, nonce, &tag));
    tag.zeroize();
    let outcome = match expected {
        Ok(expected) if crate::compare::constant_time_eq(expected.iter(), mic.iter()) => Ok(()),
        Ok(_) => Err(CcmError::InvalidMic),
        Err(error) => Err(CcmError::Engine(error)),
    };
    if outcome.is_err() {
        body.zeroize();
    }
    outcome
}

/// The lengths of one admitted call, each fitting its two-byte field: the
/// associated data's (below `0xff00`) and the body's.
#[derive(Clone, Copy)]
struct Lengths {
    aad: u16,
    body: u16,
}

/// Check the MIC length at compile time and the lengths of this call.
fn admit<const M: usize, S>(aad: &[u8], body: &[u8]) -> Result<Lengths, CcmError<S>> {
    const {
        if M < 4 || M > BLOCK || !M.is_multiple_of(2) {
            panic!("a CCM MIC is 4, 6, 8, 10, 12, 14 or 16 bytes");
        }
    }
    let aad = u16::try_from(aad.len())
        .ok()
        .filter(|length| usize::from(*length) < AAD_LIMIT)
        .ok_or(CcmError::AadTooLong)?;
    let body = u16::try_from(body.len()).map_err(|_| CcmError::BodyTooLong)?;
    Ok(Lengths { aad, body })
}

/// The unmasked CBC-MAC `T` into `tag` over `B_0`, the encoded associated
/// data and the plaintext `body`, each segment zero-padded to a block.
fn cbc_mac<E: BlockEngine, const M: usize>(
    engine: &mut E,
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    body: &[u8],
    lengths: Lengths,
    tag: &mut Block,
) -> Result<(), E::Stop> {
    // The flags' MIC field `(M - 2) / 2` (RFC 3610 §2.2), three bits,
    // counted at build.
    let mic_field: u8 = const {
        let mut field = 0_u8;
        let mut octets = 2;
        while octets < M {
            field = match field.checked_add(1) {
                Some(field) => field,
                None => panic!("a CCM MIC field fits three bits"),
            };
            octets = octets.saturating_add(2);
        }
        field
    };
    let adata = if aad.is_empty() { 0 } else { 0x40 };
    let flags = adata | (mic_field << 3) | LENGTH_FIELD;
    *tag = block(flags, nonce, lengths.body);
    engine.encrypt(tag)?;
    if !aad.is_empty() {
        absorb(engine, tag, lengths.aad.to_be_bytes().iter().chain(aad))?;
    }
    absorb(engine, tag, body.iter())
}

/// Absorb `bytes` into the chaining value `state` a block at a time, the
/// last block zero-padded.
fn absorb<'a, E: BlockEngine>(
    engine: &mut E,
    state: &mut Block,
    mut bytes: impl Iterator<Item = &'a u8>,
) -> Result<(), E::Stop> {
    loop {
        let mut absorbed = false;
        for (out, byte) in state.iter_mut().zip(&mut bytes) {
            *out ^= byte;
            absorbed = true;
        }
        if !absorbed {
            return Ok(());
        }
        engine.encrypt(state)?;
    }
}

/// Encrypt (or decrypt) `body` in place with the counter blocks `A_1..`, a
/// block of the body at a time.
fn counter_pass<E: BlockEngine>(
    engine: &mut E,
    nonce: &[u8; NONCE_LEN],
    body: &mut [u8],
) -> Result<(), E::Stop> {
    let mut stream: Block = [0; BLOCK];
    let mut result = Ok(());
    let mut bytes = body.iter_mut();
    // An admitted body is at most 65535 bytes: at most 4096 counter blocks,
    // so the counter never wraps.
    let mut counter: u16 = 0;
    while bytes.len() > 0 {
        counter = counter.wrapping_add(1);
        stream = block(LENGTH_FIELD, nonce, counter);
        if let Err(error) = engine.encrypt(&mut stream) {
            result = Err(error);
            break;
        }
        // The key stream first: a block of it takes at most a block of the
        // body and leaves the rest.
        for (key, byte) in stream.iter().zip(&mut bytes) {
            *byte ^= key;
        }
    }
    stream.zeroize();
    result
}

/// The MIC: the first `M` bytes of `T` masked with `E(K, A_0)`.
fn mask<E: BlockEngine, const M: usize>(
    engine: &mut E,
    nonce: &[u8; NONCE_LEN],
    tag: &Block,
) -> Result<[u8; M], E::Stop> {
    let mut s0 = block(LENGTH_FIELD, nonce, 0);
    let result = engine.encrypt(&mut s0).map(|()| {
        let mut mic = [0; M];
        for ((out, value), mask) in mic.iter_mut().zip(tag.iter()).zip(s0.iter()) {
            *out = value ^ mask;
        }
        mic
    });
    s0.zeroize();
    result
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

#[cfg(test)]
mod tests;
