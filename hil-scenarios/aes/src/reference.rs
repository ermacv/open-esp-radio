//! The host's reference: RFC 3394 key wrap over RustCrypto's AES-128,
//! written from the RFC's §2.2.1 index-based algorithm, and the RFC's own
//! test vector.

use aes::cipher::{BlockEncrypt, KeyInit};

/// RFC 3394 §4.1: wrap 128 bits of key data with a 128-bit KEK.
pub const RFC3394_4_1_KEK: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];
pub const RFC3394_4_1_KEY_DATA: [u8; 16] = [
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];
pub const RFC3394_4_1_WRAPPED: [u8; 24] = [
    0x1f, 0xa6, 0x8b, 0x0a, 0x81, 0x12, 0xb4, 0x47, 0xae, 0xf3, 0x4b, 0xd8, 0xfb, 0x5a, 0x7b, 0x82,
    0x9d, 0x3e, 0x86, 0x23, 0x71, 0xd2, 0xcf, 0xe5,
];

/// The default initial value (RFC 3394 §2.2.3.1).
const IV: [u8; 8] = [0xa6; 8];

/// Wrap `data`, `n` 64-bit blocks with `n >= 2`, under `kek` (RFC 3394 §2.2.1).
pub fn wrap(kek: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let cipher = aes::Aes128::new(kek.into());
    let n = data.len() / 8;
    let mut a = IV;
    let mut r: Vec<[u8; 8]> = data
        .chunks_exact(8)
        .map(|block| block.try_into().expect("an 8-byte chunk"))
        .collect();
    for j in 0..6 {
        for (i, register) in r.iter_mut().enumerate() {
            let mut b = [0; 16];
            b[..8].copy_from_slice(&a);
            b[8..].copy_from_slice(register);
            cipher.encrypt_block((&mut b).into());
            let t = (n * j + i + 1) as u64;
            a.copy_from_slice(&b[..8]);
            for (byte, counter) in a.iter_mut().zip(t.to_be_bytes()) {
                *byte ^= counter;
            }
            register.copy_from_slice(&b[8..]);
        }
    }
    let mut wrapped = a.to_vec();
    for register in r {
        wrapped.extend_from_slice(&register);
    }
    wrapped
}

/// The reference cases: key data of 16, 24 and 32 bytes under three KEKs,
/// drawn from a fixed xorshift sequence so that every run sends the same.
pub fn cases() -> Vec<([u8; 16], Vec<u8>)> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as u8
    };
    let mut cases = Vec::new();
    for _ in 0..3 {
        let kek: [u8; 16] = std::array::from_fn(|_| next());
        for length in [16, 24, 32] {
            cases.push((kek, (0..length).map(|_| next()).collect()));
        }
    }
    cases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_wrap_reproduces_the_rfc_vector() {
        assert_eq!(
            wrap(&RFC3394_4_1_KEK, &RFC3394_4_1_KEY_DATA),
            RFC3394_4_1_WRAPPED
        );
    }

    #[test]
    fn every_reference_case_fits_one_request_and_differs() {
        let cases = cases();
        assert_eq!(cases.len(), 9);
        for (kek, data) in &cases {
            assert!(wrap(kek, data).len() <= oer_services_aes::MAX_WRAPPED);
        }
        assert_ne!(cases[0].0, cases[3].0);
        assert_eq!(cases, super::cases());
    }
}
