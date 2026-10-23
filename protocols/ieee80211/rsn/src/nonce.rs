//! A separate, boot-seeded CSPRNG owner for supplicant nonces on P1.

use chacha20::{ChaCha20, KeyIvInit};
use cipher::StreamCipher;
use zeroize::Zeroizing;

/// A ChaCha20 stream seeded once with 256 bits from the composition's sole
/// hardware entropy owner. The stream is never reset for a new association.
/// Its cipher state and buffered output are erased on drop.
pub struct SNonceSource(ChaCha20);

impl SNonceSource {
    /// `fill` must supply 256 bits of fresh cryptographic entropy. It is
    /// called exactly once, before the hardware owner moves to P2.
    pub fn from_entropy(fill: impl FnOnce(&mut [u8; 32])) -> Self {
        let mut seed = Zeroizing::new([0; 32]);
        fill(&mut seed);
        // The nonce is fixed within this stream; every boot has a fresh key.
        Self(ChaCha20::new((&*seed).into(), (&[0; 12]).into()))
    }

    /// Fails closed if the stream's finite counter is exhausted. The caller
    /// must stop the association rather than substitute a repeated nonce.
    pub fn next_nonce(&mut self) -> Result<[u8; 32], chacha20::cipher::StreamCipherError> {
        let mut nonce = [0; 32];
        self.0.try_apply_keystream(&mut nonce)?;
        Ok(nonce)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_once_and_advances_across_handshakes() {
        let mut seeds = 0;
        let mut source = SNonceSource::from_entropy(|seed| {
            seeds += 1;
            seed.fill(0x42);
        });
        let first = source.next_nonce().unwrap();
        let second = source.next_nonce().unwrap();
        assert_eq!(seeds, 1);
        assert_ne!(first, second);
        assert_ne!(first, [0; 32]);
        fn wipes_on_drop<T: zeroize::ZeroizeOnDrop>() {}
        wipes_on_drop::<ChaCha20>();
    }
}
