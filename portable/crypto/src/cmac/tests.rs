use super::*;
use crate::test_engine::{Engine, REFUSAL};
use cmac::Mac;
use std::vec::Vec;

/// RFC 4493 §4 key.
const RFC4493_KEY: [u8; 16] = [
    0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c,
];

fn tag(key: &[u8; 16], pieces: &[&[u8]]) -> Block {
    let mut mac = Cmac::new(Engine::new(key)).unwrap();
    for piece in pieces {
        mac.update(piece).unwrap();
    }
    mac.finalize().unwrap()
}

#[test]
fn rfc4493_examples_1_and_2() {
    assert_eq!(
        tag(&RFC4493_KEY, &[]),
        [
            0xbb, 0x1d, 0x69, 0x29, 0xe9, 0x59, 0x37, 0x28, 0x7f, 0xa3, 0x7d, 0x12, 0x9b, 0x75,
            0x67, 0x46
        ]
    );
    let message = [
        0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17,
        0x2a,
    ];
    assert_eq!(
        tag(&RFC4493_KEY, &[&message]),
        [
            0x07, 0x0a, 0x16, 0xb4, 0x6b, 0x4d, 0x41, 0x44, 0xf7, 0x9b, 0xdd, 0x9d, 0xd0, 0x4a,
            0x28, 0x7c
        ]
    );
}

#[test]
fn every_length_and_split_matches_rustcrypto_cmac() {
    let key = *b"fedcba9876543210";
    for length in [0, 1, 15, 16, 17, 31, 32, 33, 64, 99, 121, 512] {
        let message: Vec<u8> = (0..length).map(|i| (i * 13) as u8).collect();
        let mut reference = <cmac::Cmac<aes::Aes128> as cmac::digest::KeyInit>::new(&key.into());
        reference.update(&message);
        let expected: Block = reference.finalize().into_bytes().into();
        assert_eq!(tag(&key, &[&message]), expected, "{length}");
        for split in [0, 1, 7, 16, 17, length / 2, length] {
            let split = split.min(length);
            let (head, tail) = message.split_at(split);
            assert_eq!(tag(&key, &[head, &[], tail]), expected, "{length} {split}");
        }
    }
}

#[test]
fn verification_compares_the_truncated_tag() {
    let message = b"a deauthentication body";
    let full = tag(&RFC4493_KEY, &[message]);
    let mut first: [u8; 8] = full[..8].try_into().unwrap();
    let verify = |expected: &[u8; 8]| {
        let mut mac = Cmac::new(Engine::new(&RFC4493_KEY)).unwrap();
        mac.update(message).unwrap();
        mac.verify_truncated(expected).unwrap()
    };
    assert!(verify(&first));
    first[0] ^= 0x80;
    assert!(!verify(&first));
}

#[test]
fn the_first_refusal_poisons_the_computation() {
    assert_eq!(
        Cmac::new(Engine::refusing_from(&RFC4493_KEY, 0)).err(),
        Some(REFUSAL)
    );
    // L, then the first 16 bytes once more arrive.
    let mut mac = Cmac::new(Engine::refusing_from(&RFC4493_KEY, 1)).unwrap();
    assert_eq!(mac.update(&[1; 16]), Ok(()));
    assert_eq!(mac.update(&[2]), Err(REFUSAL));
    assert_eq!(mac.update(&[]), Err(REFUSAL));
    assert_eq!(mac.finalize(), Err(REFUSAL));
    let mut mac = Cmac::new(Engine::refusing_from(&RFC4493_KEY, 1)).unwrap();
    mac.update(&[3; 5]).unwrap();
    assert_eq!(mac.verify_truncated(&[0; 8]), Err(REFUSAL));
}

#[test]
fn the_subkey_step_adds_rb_only_after_a_carry() {
    let mut high = [0; 16];
    high[0] = 0x80;
    let mut expected = [0; 16];
    expected[15] = 0x87;
    assert_eq!(double(&high), expected);
    let mut low = [0; 16];
    low[15] = 0x41;
    let mut shifted = [0; 16];
    shifted[15] = 0x82;
    assert_eq!(double(&low), shifted);
}
