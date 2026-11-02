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

#[test]
fn ccm_as_a_cbc_mac_and_a_ctr_pass_matches_the_block_by_block_seal() {
    let nonce = [9_u8; NONCE];
    for (aad_length, body_length) in [(22, 1), (22, 64), (28, 512), (30, 1500), (24, 17)] {
        let aad: Vec<u8> = (0..aad_length).map(|i| (i * 5) as u8).collect();
        let plaintext: Vec<u8> = (0..body_length).map(|i| (i * 11) as u8).collect();
        let mut by_blocks = plaintext.clone();
        let mic = ccm_seal(&mut software(), &nonce, &aad, &mut by_blocks);
        let mut by_passes = plaintext.clone();
        let passes_mic =
            ccm_seal_passes(&mut CipherPasses(software()), &nonce, &aad, &mut by_passes);
        assert_eq!(by_passes, by_blocks, "{aad_length} {body_length}");
        assert_eq!(passes_mic, mic, "{aad_length} {body_length}");
    }
}

#[test]
fn cmac_as_one_cbc_pass_matches_the_block_by_block_cmac() {
    for length in [0, 1, 15, 16, 17, 64, 100, 512] {
        let message: Vec<u8> = (0..length).map(|i| (i * 7) as u8).collect();
        assert_eq!(
            cmac_passes(&mut CipherPasses(software()), &message),
            cmac(&mut software(), &message),
            "{length}"
        );
    }
}

#[test]
fn ctr_increments_the_last_32_bits_big_endian_and_wraps() {
    let mut block = [0_u8; 16];
    block[12..].copy_from_slice(&[0, 0, 0xff, 0xff]);
    increment32(&mut block);
    assert_eq!(block[12..], [0, 1, 0, 0]);
    block[12..].copy_from_slice(&[0xff; 4]);
    block[11] = 7;
    increment32(&mut block);
    assert_eq!((block[11], &block[12..]), (7, &[0_u8; 4][..]));
}
