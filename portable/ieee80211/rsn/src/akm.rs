//! Authentication and key management suites negotiated in the RSN element.
//!
//! The four-way handshake, EAPOL-Key framing, AES key wrap and key
//! installation are shared by every suite. A suite selects only its key
//! descriptor version, pairwise key expansion and EAPOL-Key MIC; each property
//! is an exhaustive match so adding a suite forces every dependent decision.

use aes::Aes128;
use cmac::Cmac;
use hmac::{Hmac, Mac};
use oer_ieee80211_mac::security::rsn::Akm;
use sha1::Sha1;
use zeroize::Zeroize;

use crate::{RSN_KCK_LEN, RSN_PTK_LEN};

const PTK_EXPANSION_LABEL: &[u8] = b"Pairwise key expansion";

/// Length of every EAPOL-Key MIC produced by a supported suite.
pub const RSN_MIC_LEN: usize = 16;

/// The per-suite key hierarchy and EAPOL-Key integrity of an [`Akm`].
pub(crate) trait AkmKeys {
    /// Key Information descriptor version carried by every EAPOL-Key frame.
    fn key_descriptor_version(self) -> u8;
    /// Expand a PMK over the canonical address/nonce context into a PTK.
    fn expand_ptk(self, pmk: &[u8; 32], context: &[u8; 76], output: &mut [u8; RSN_PTK_LEN]);
    fn mic(self, kck: &[u8; RSN_KCK_LEN]) -> EapolMic;
}

impl AkmKeys for Akm {
    fn key_descriptor_version(self) -> u8 {
        match self {
            Self::Psk => 2,
            Self::PskSha256 => 3,
            Self::Sae => 0,
        }
    }

    fn expand_ptk(self, pmk: &[u8; 32], context: &[u8; 76], output: &mut [u8; RSN_PTK_LEN]) {
        match self {
            Self::Psk => prf_sha1(pmk, context, output),
            Self::PskSha256 | Self::Sae => {
                crate::kdf::kdf_sha256(pmk, PTK_EXPANSION_LABEL, context, output);
            }
        }
    }

    fn mic(self, kck: &[u8; RSN_KCK_LEN]) -> EapolMic {
        match self {
            Self::Psk => EapolMic::HmacSha1(
                Hmac::<Sha1>::new_from_slice(kck).expect("KCK length is always accepted by HMAC"),
            ),
            Self::PskSha256 | Self::Sae => EapolMic::AesCmac(
                Cmac::<Aes128>::new_from_slice(kck).expect("a 16-byte KCK is an AES-128 key"),
            ),
        }
    }
}

/// Incremental EAPOL-Key MIC of one suite.
#[expect(
    clippy::large_enum_variant,
    reason = "a MIC lives for one frame on the stack; the AES-CMAC key schedule is its whole state"
)]
pub(crate) enum EapolMic {
    HmacSha1(Hmac<Sha1>),
    AesCmac(Cmac<Aes128>),
}

impl EapolMic {
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::HmacSha1(mac) => mac.update(bytes),
            Self::AesCmac(mac) => mac.update(bytes),
        }
    }

    pub(crate) fn finalize(self) -> [u8; RSN_MIC_LEN] {
        match self {
            Self::HmacSha1(mac) => {
                let mut digest = mac.finalize().into_bytes();
                let mut mic = [0; RSN_MIC_LEN];
                mic.copy_from_slice(&digest[..RSN_MIC_LEN]);
                digest.zeroize();
                mic
            }
            Self::AesCmac(mac) => {
                let mut tag = mac.finalize().into_bytes();
                let mut mic = [0; RSN_MIC_LEN];
                mic.copy_from_slice(&tag);
                tag.zeroize();
                mic
            }
        }
    }

    /// Constant-time comparison against a received MIC.
    pub(crate) fn verify(self, mic: &[u8; RSN_MIC_LEN]) -> bool {
        match self {
            Self::HmacSha1(mac) => mac.verify_truncated_left(mic).is_ok(),
            Self::AesCmac(mac) => mac.verify_slice(mic).is_ok(),
        }
    }
}

/// IEEE 802.11 PRF-384 with HMAC-SHA1 and the pairwise expansion label.
fn prf_sha1(pmk: &[u8; 32], context: &[u8; 76], ptk: &mut [u8; RSN_PTK_LEN]) {
    let mut written = 0;
    let mut counter = 0_u8;
    while written < ptk.len() {
        let mut mac =
            Hmac::<Sha1>::new_from_slice(pmk).expect("PMK length is always accepted by HMAC");
        mac.update(PTK_EXPANSION_LABEL);
        mac.update(&[0]);
        mac.update(context);
        mac.update(&[counter]);
        let mut block = mac.finalize().into_bytes();
        let count = core::cmp::min(block.len(), ptk.len() - written);
        ptk[written..written + count].copy_from_slice(&block[..count]);
        block.zeroize();
        written += count;
        counter = counter.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expanded(akm: Akm, pmk: &[u8; 32], context: &[u8; 76]) -> [u8; RSN_PTK_LEN] {
        let mut bytes = [0; RSN_PTK_LEN];
        akm.expand_ptk(pmk, context, &mut bytes);
        bytes
    }

    #[test]
    fn suite_selectors_round_trip_and_unknown_suites_are_unsupported() {
        assert_eq!(
            Akm::from_suite_selector(Akm::Psk.suite_selector()),
            Some(Akm::Psk)
        );
        assert_eq!(Akm::Psk.suite_selector(), [0x00, 0x0f, 0xac, 2]);
        assert_eq!(
            Akm::from_suite_selector([0x00, 0x0f, 0xac, 6]),
            Some(Akm::PskSha256)
        );
        // FT-SAE and SAE-EXT-KEY stay unsupported.
        assert_eq!(Akm::from_suite_selector([0x00, 0x0f, 0xac, 9]), None);
        assert_eq!(Akm::from_suite_selector([0x00, 0x0f, 0xac, 24]), None);
        assert_eq!(Akm::from_suite_selector([0x00, 0x50, 0xf2, 2]), None);
    }

    #[test]
    fn mic_verification_accepts_only_the_finalized_value() {
        let kck = [7; RSN_KCK_LEN];
        let mut mic = Akm::Psk.mic(&kck);
        mic.update(b"frame");
        let value = mic.finalize();
        let mut check = Akm::Psk.mic(&kck);
        check.update(b"frame");
        assert!(check.verify(&value));
        let mut tampered = Akm::Psk.mic(&kck);
        tampered.update(b"framf");
        assert!(!tampered.verify(&value));
    }

    #[test]
    fn psk_sha256_expands_with_the_sha256_kdf_and_signs_with_aes_cmac() {
        let pmk: [u8; 32] = core::array::from_fn(|index| index as u8);
        let context: [u8; 76] = core::array::from_fn(|index| 100 + index as u8);
        // Computed independently with Python's hmac/hashlib.
        let expected = [
            0xf8, 0xbe, 0xe5, 0x0e, 0x6e, 0x9c, 0xfe, 0x26, 0xa3, 0xd5, 0x6c, 0x3b, 0x7f, 0x72,
            0x81, 0x59, 0x7f, 0xda, 0xe9, 0x91, 0x87, 0x90, 0xe8, 0xf1, 0xd4, 0xf3, 0xeb, 0x09,
            0x11, 0xfa, 0x6b, 0x7b, 0x86, 0xea, 0xca, 0xbc, 0x7a, 0xde, 0xcc, 0x4a, 0xe7, 0xd5,
            0x81, 0xc5, 0xac, 0xb1, 0x5c, 0xf9,
        ];
        assert_eq!(expanded(Akm::PskSha256, &pmk, &context), expected);
        assert_eq!(Akm::PskSha256.key_descriptor_version(), 3);

        // Computed independently with Python's cryptography CMAC.
        let mut mic = Akm::PskSha256.mic(&[7; RSN_KCK_LEN]);
        mic.update(b"eapol-frame-bytes");
        assert_eq!(
            mic.finalize(),
            [
                0xc9, 0x54, 0x6e, 0xe7, 0xb6, 0xed, 0x61, 0x8c, 0x15, 0x91, 0x63, 0xcb, 0x3a, 0x83,
                0xe9, 0xb4
            ]
        );
    }

    #[test]
    fn sae_keys_like_psk_sha256_under_the_akm_defined_descriptor() {
        assert_eq!(
            Akm::from_suite_selector([0x00, 0x0f, 0xac, 8]),
            Some(Akm::Sae)
        );
        assert_eq!(Akm::Sae.suite_selector(), [0x00, 0x0f, 0xac, 8]);
        assert_eq!(Akm::Sae.key_descriptor_version(), 0);
        let pmk: [u8; 32] = core::array::from_fn(|index| index as u8);
        let context: [u8; 76] = core::array::from_fn(|index| 100 + index as u8);
        assert_eq!(
            expanded(Akm::Sae, &pmk, &context),
            expanded(Akm::PskSha256, &pmk, &context)
        );
        let mut sae = Akm::Sae.mic(&[7; RSN_KCK_LEN]);
        sae.update(b"eapol-frame-bytes");
        let mut psk_sha256 = Akm::PskSha256.mic(&[7; RSN_KCK_LEN]);
        psk_sha256.update(b"eapol-frame-bytes");
        assert_eq!(sae.finalize(), psk_sha256.finalize());
    }
}
