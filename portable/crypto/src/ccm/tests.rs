use super::*;
use crate::test_engine::{Engine, REFUSAL};
use ccm::{
    Ccm,
    aead::{AeadInPlace, KeyInit},
    consts::{U4, U8, U13, U16},
};
use std::{vec, vec::Vec};

const KEY: [u8; 16] = *b"0123456789abcdef";
const NONCE: [u8; NONCE_LEN] = [0x5a; NONCE_LEN];

fn data(length: usize, step: usize) -> Vec<u8> {
    (0..length).map(|i| (i * step) as u8).collect()
}

const LENGTHS: [(usize, usize); 10] = [
    (22, 0),
    (22, 1),
    (0, 15),
    (14, 16),
    (30, 17),
    (28, 64),
    (24, 100),
    (30, 512),
    (22, 1500),
    (300, 33),
];

macro_rules! matches_rustcrypto {
    ($name:ident, $m:literal, $tag:ty) => {
        #[test]
        fn $name() {
            type Reference = Ccm<aes::Aes128, $tag, U13>;
            for (aad_length, body_length) in LENGTHS {
                let aad = data(aad_length, 5);
                let plaintext = data(body_length, 7);
                let mut ours = plaintext.clone();
                let mic = seal::<_, $m>(&mut Engine::new(&KEY), &NONCE, &aad, &mut ours).unwrap();
                let mut theirs = plaintext.clone();
                let tag = Reference::new(&KEY.into())
                    .encrypt_in_place_detached(&NONCE.into(), &aad, &mut theirs)
                    .unwrap();
                assert_eq!(ours, theirs, "{aad_length} {body_length}");
                assert_eq!(mic.as_slice(), tag.as_slice(), "{aad_length} {body_length}");
                open::<_, $m>(&mut Engine::new(&KEY), &NONCE, &aad, &mut ours, &mic).unwrap();
                assert_eq!(ours, plaintext, "{aad_length} {body_length}");
            }
        }
    };
}

matches_rustcrypto!(an_eight_byte_mic_seals_and_opens_as_rustcrypto_ccm, 8, U8);
matches_rustcrypto!(a_four_byte_mic_seals_and_opens_as_rustcrypto_ccm, 4, U4);
matches_rustcrypto!(
    a_sixteen_byte_mic_seals_and_opens_as_rustcrypto_ccm,
    16,
    U16
);

#[test]
fn a_changed_mic_aad_or_body_erases_the_body_and_fails() {
    let aad = data(22, 3);
    let plaintext = data(40, 9);
    let mut sealed = plaintext.clone();
    let mic = seal::<_, 8>(&mut Engine::new(&KEY), &NONCE, &aad, &mut sealed).unwrap();

    let mut bad_mic = mic;
    bad_mic[7] ^= 1;
    let mut body = sealed.clone();
    assert_eq!(
        open::<_, 8>(&mut Engine::new(&KEY), &NONCE, &aad, &mut body, &bad_mic),
        Err(CcmError::InvalidMic)
    );
    assert!(body.iter().all(|byte| *byte == 0));

    let mut bad_aad = aad.clone();
    bad_aad[0] ^= 1;
    let mut body = sealed.clone();
    assert_eq!(
        open::<_, 8>(&mut Engine::new(&KEY), &NONCE, &bad_aad, &mut body, &mic),
        Err(CcmError::InvalidMic)
    );

    let mut body = sealed.clone();
    body[39] ^= 1;
    assert_eq!(
        open::<_, 8>(&mut Engine::new(&KEY), &NONCE, &aad, &mut body, &mic),
        Err(CcmError::InvalidMic)
    );
    assert!(body.iter().all(|byte| *byte == 0));
}

#[test]
fn lengths_beyond_the_encodings_are_refused_before_any_block() {
    let mut engine = Engine::new(&KEY);
    let mut body = vec![0; usize::from(u16::MAX) + 1];
    assert_eq!(
        seal::<_, 8>(&mut engine, &NONCE, &[], &mut body),
        Err(CcmError::BodyTooLong)
    );
    let aad = vec![0; 0xff00];
    assert_eq!(
        seal::<_, 8>(&mut engine, &NONCE, &aad, &mut [0; 4]),
        Err(CcmError::AadTooLong)
    );
    assert_eq!(engine.blocks, 0);
}

#[test]
fn a_refused_block_erases_the_body_at_every_stage() {
    let aad = data(22, 3);
    let plaintext = data(40, 9);
    let mut sealed = plaintext.clone();
    let mic = seal::<_, 8>(&mut Engine::new(&KEY), &NONCE, &aad, &mut sealed).unwrap();
    // B0, two AAD blocks, three body blocks for the MAC; three counter blocks
    // and A0: ten blocks in all.
    let mut counting = Engine::new(&KEY);
    seal::<_, 8>(&mut counting, &NONCE, &aad, &mut plaintext.clone()).unwrap();
    assert_eq!(counting.blocks, 10);
    for refuse_from in 0..10 {
        let mut body = plaintext.clone();
        assert_eq!(
            seal::<_, 8>(
                &mut Engine::refusing_from(&KEY, refuse_from),
                &NONCE,
                &aad,
                &mut body
            ),
            Err(CcmError::Engine(REFUSAL)),
            "seal {refuse_from}"
        );
        if refuse_from >= 6 {
            assert!(body.iter().all(|byte| *byte == 0), "seal {refuse_from}");
        }
        let mut body = sealed.clone();
        assert_eq!(
            open::<_, 8>(
                &mut Engine::refusing_from(&KEY, refuse_from),
                &NONCE,
                &aad,
                &mut body,
                &mic
            ),
            Err(CcmError::Engine(REFUSAL)),
            "open {refuse_from}"
        );
        assert!(body.iter().all(|byte| *byte == 0), "open {refuse_from}");
    }
}
