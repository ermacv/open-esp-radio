//! The IEEE 802.11 key derivation function with SHA-256.
//!
//! SOURCE: IEEE Std 802.11-2020 12.7.1.6.2 (KDF).

use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroize;

/// The IEEE 802.11 KDF-Hash-Length with SHA-256: HMAC blocks over a 16-bit
/// counter, the label, the context and the output length in bits.
pub(crate) fn kdf_sha256(key: &[u8], label: &[u8], context: &[u8], output: &mut [u8]) {
    let bits = (output.len() * 8) as u16;
    for (counter, chunk) in (1_u16..).zip(output.chunks_mut(32)) {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts every key length");
        mac.update(&counter.to_le_bytes());
        mac.update(label);
        mac.update(context);
        mac.update(&bits.to_le_bytes());
        let mut block = mac.finalize().into_bytes();
        chunk.copy_from_slice(&block[..chunk.len()]);
        block.zeroize();
    }
}
