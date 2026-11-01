use super::*;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use ccm::aead::AeadInPlace;

struct Software(aes::Aes128);

impl BlockCipher for Software {
    fn encrypt_block(&mut self, block: &mut Block) {
        self.0.encrypt_block(block.into());
    }
    fn decrypt_block(&mut self, block: &mut Block) {
        self.0.decrypt_block(block.into());
    }
}

const KEY: [u8; 16] = *b"0123456789abcdef";

fn software() -> Software {
    Software(aes::Aes128::new(&KEY.into()))
}

#[test]
fn ccm_over_a_block_cipher_matches_ccmp_s_rustcrypto_ccm() {
    type CcmpCcm = ccm::Ccm<aes::Aes128, ccm::consts::U8, ccm::consts::U13>;
    let nonce = [7_u8; NONCE];
    for (aad_length, body_length) in [(22, 0), (22, 1), (28, 64), (30, 512), (22, 1500), (24, 17)] {
        let aad: Vec<u8> = (0..aad_length).map(|i| i as u8).collect();
        let plaintext: Vec<u8> = (0..body_length).map(|i| (i * 7) as u8).collect();
        let mut ours = plaintext.clone();
        let mic = ccm_seal(&mut software(), &nonce, &aad, &mut ours);
        let mut theirs = plaintext.clone();
        let tag = CcmpCcm::new(&KEY.into())
            .encrypt_in_place_detached(&nonce.into(), &aad, &mut theirs)
            .unwrap();
        assert_eq!(ours, theirs, "{aad_length} {body_length}");
        assert_eq!(mic.as_slice(), tag.as_slice(), "{aad_length} {body_length}");
    }
}

#[test]
fn cmac_over_a_block_cipher_matches_bip_s_rustcrypto_cmac() {
    use cmac::Mac;
    for length in [0, 1, 15, 16, 17, 32, 64, 100, 512] {
        let message: Vec<u8> = (0..length).map(|i| (i * 3) as u8).collect();
        let ours = cmac(&mut software(), &message);
        let mut theirs = <cmac::Cmac<aes::Aes128> as KeyInit>::new(&KEY.into());
        theirs.update(&message);
        assert_eq!(
            ours.as_slice(),
            theirs.finalize().into_bytes().as_slice(),
            "{length}"
        );
    }
}

#[test]
fn statistics_keep_the_extremes_and_the_mean() {
    let mut stats = Stats::default();
    assert_eq!(stats.mean(), 0);
    for cycles in [30, 10, 20] {
        stats.add(cycles);
    }
    assert_eq!(
        (stats.runs, stats.min, stats.max, stats.mean()),
        (3, 10, 30, 20)
    );
}

#[test]
fn a_case_line_names_its_mode_operation_and_cycles() {
    let mut stats = Stats::default();
    stats.add(123);
    stats.add(7);
    let line = Line::case("flag", "block", 16, &stats);
    assert_eq!(
        core::str::from_utf8(line.as_bytes()).unwrap(),
        "OER-PROBE aes-timing flag block bytes=16 runs=2 min=7 mean=65 max=123\r\n"
    );
}
