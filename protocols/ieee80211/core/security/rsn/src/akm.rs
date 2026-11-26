//! Authentication and key management suites negotiated in the RSN element.
//!
//! The four-way handshake, EAPOL-Key framing, AES key wrap and key
//! installation are shared by every suite. A suite selects only its key
//! descriptor version, pairwise key expansion and EAPOL-Key MIC; each property
//! is an exhaustive match so adding a suite forces every dependent decision.

use hmac::Mac;
use oer_portable_crypto::block::Aes128Engine;
use oer_protocols_ieee80211_wire::security::rsn::Akm;
use zeroize::Zeroize;

use crate::{Kck, Pmk};

const PTK_EXPANSION_LABEL: &[u8] = b"Pairwise key expansion";

/// Length of every EAPOL-Key MIC produced by a supported suite.
pub const RSN_MIC_LEN: usize = 16;

/// The per-suite key hierarchy and EAPOL-Key integrity of an [`Akm`].
pub(crate) trait AkmKeys {
    /// Key Information descriptor version carried by every EAPOL-Key frame.
    fn key_descriptor_version(self) -> oer_portable_wire::U8<3>;
    /// Expand a PMK over the canonical address/nonce context into a PTK.
    fn expand_ptk(self, pmk: &Pmk, context: &[u8; 76], output: &mut [[u8; 16]; 3]);
    /// The suite's EAPOL-Key MIC under `kck` of the message whose pieces
    /// `message` gives. An AES-CMAC suite runs on `engine`, which holds the
    /// KCK until the MIC is done.
    fn mic<'m, E: Aes128Engine>(
        self,
        kck: &Kck,
        engine: &mut E,
        message: impl IntoIterator<Item = &'m [u8]>,
    ) -> Result<[u8; RSN_MIC_LEN], E::Stop>;
    /// Compare the message's MIC with `mic` in constant time: it verifies,
    /// or the failure.
    fn verify_mic<'m, E: Aes128Engine>(
        self,
        kck: &Kck,
        engine: &mut E,
        message: impl IntoIterator<Item = &'m [u8]>,
        mic: &[u8; RSN_MIC_LEN],
    ) -> Result<(), crate::MicFailure<E::Stop>>;
}

impl AkmKeys for Akm {
    fn key_descriptor_version(self) -> oer_portable_wire::U8<3> {
        // SOURCE: IEEE Std 802.11-2020, 12.7.2 (Key Descriptor Version: 2 for
        // HMAC-SHA-1-128 with AES key wrap, 3 for AES-128-CMAC, 0 when the
        // AKM defines the algorithms).
        match self {
            Self::Psk => oer_portable_wire::U8::constant::<2>(),
            Self::PskSha256 => oer_portable_wire::U8::constant::<3>(),
            Self::Sae => oer_portable_wire::U8::ZERO,
        }
    }

    fn expand_ptk(self, pmk: &Pmk, context: &[u8; 76], output: &mut [[u8; 16]; 3]) {
        match self {
            Self::Psk => prf_sha1(pmk, context, output),
            Self::PskSha256 | Self::Sae => {
                crate::kdf::kdf_sha256(pmk.as_bytes(), PTK_EXPANSION_LABEL, context, output);
            }
        }
    }

    fn mic<'m, E: Aes128Engine>(
        self,
        kck: &Kck,
        engine: &mut E,
        message: impl IntoIterator<Item = &'m [u8]>,
    ) -> Result<[u8; RSN_MIC_LEN], E::Stop> {
        match self {
            Self::Psk => {
                let mut mac = crate::mac::sha1(kck.as_bytes());
                for piece in message {
                    mac.update(piece);
                }
                let mut digest = mac.finalize().into_bytes();
                let mut mic = [0; RSN_MIC_LEN];
                for (target, source) in mic.iter_mut().zip(digest.iter()) {
                    *target = *source;
                }
                digest.zeroize();
                Ok(mic)
            }
            Self::PskSha256 | Self::Sae => {
                let keyed = engine.load(kck.aes_key())?;
                oer_portable_crypto::cmac::tag(keyed, message)
            }
        }
    }

    fn verify_mic<'m, E: Aes128Engine>(
        self,
        kck: &Kck,
        engine: &mut E,
        message: impl IntoIterator<Item = &'m [u8]>,
        mic: &[u8; RSN_MIC_LEN],
    ) -> Result<(), crate::MicFailure<E::Stop>> {
        match self {
            Self::Psk => {
                let mut mac = crate::mac::sha1(kck.as_bytes());
                for piece in message {
                    mac.update(piece);
                }
                mac.verify_truncated_left(mic)
                    .map_err(|_| crate::MicFailure::Mismatch)
            }
            Self::PskSha256 | Self::Sae => {
                let keyed = engine
                    .load(kck.aes_key())
                    .map_err(crate::MicFailure::Engine)?;
                oer_portable_crypto::cmac::verify(keyed, message, mic).map_err(
                    |error| match error {
                        oer_portable_crypto::cmac::CmacVerifyError::InvalidTag => {
                            crate::MicFailure::Mismatch
                        }
                        oer_portable_crypto::cmac::CmacVerifyError::Engine(stop) => {
                            crate::MicFailure::Engine(stop)
                        }
                    },
                )
            }
        }
    }
}

/// IEEE 802.11 PRF-384 with HMAC-SHA1 and the pairwise expansion label.
fn prf_sha1(pmk: &Pmk, context: &[u8; 76], ptk: &mut [[u8; 16]; 3]) {
    const {
        if !(3 * 20 >= crate::RSN_PTK_LEN && 2 * 20 < crate::RSN_PTK_LEN) {
            panic!("PRF-384 SHA-1 block geometry");
        }
    }
    let mut remaining = ptk.as_flattened_mut().iter_mut();
    for counter in 0_u8..3 {
        let mut mac = crate::mac::sha1(pmk.as_bytes());
        mac.update(PTK_EXPANSION_LABEL);
        mac.update(&[0]);
        mac.update(context);
        mac.update(&[counter]);
        let mut block = mac.finalize().into_bytes();
        for source in &block {
            if let Some(target) = remaining.next() {
                *target = *source;
            }
        }
        block.zeroize();
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::arithmetic_side_effects,
        clippy::unreachable
    )]
    use super::*;
    use crate::test_engine::Refusing;
    use oer_portable_aes_software::SoftwareAes128;

    fn expanded(akm: Akm, pmk: &[u8; 32], context: &[u8; 76]) -> [[u8; 16]; 3] {
        let mut bytes = [[0; 16]; 3];
        akm.expand_ptk(&Pmk::from_bytes(*pmk), context, &mut bytes);
        bytes
    }

    #[test]
    fn suite_selectors_round_trip_and_unknown_suites_are_unsupported() {
        assert_eq!(
            Akm::from_suite_selector(Akm::Psk.suite_selector()),
            Some(Akm::Psk)
        );
        assert_eq!(
            Akm::Psk.suite_selector(),
            oer_protocols_ieee80211_wire::security::rsn::AkmSuite::from_octets([
                0x00, 0x0f, 0xac, 2
            ])
        );
        assert_eq!(
            Akm::from_suite_selector(
                oer_protocols_ieee80211_wire::security::rsn::AkmSuite::from_octets([
                    0x00, 0x0f, 0xac, 6
                ])
            ),
            Some(Akm::PskSha256)
        );
        // FT-SAE and SAE-EXT-KEY stay unsupported.
        assert_eq!(
            Akm::from_suite_selector(
                oer_protocols_ieee80211_wire::security::rsn::AkmSuite::from_octets([
                    0x00, 0x0f, 0xac, 9
                ])
            ),
            None
        );
        assert_eq!(
            Akm::from_suite_selector(
                oer_protocols_ieee80211_wire::security::rsn::AkmSuite::from_octets([
                    0x00, 0x0f, 0xac, 24
                ])
            ),
            None
        );
        assert_eq!(
            Akm::from_suite_selector(
                oer_protocols_ieee80211_wire::security::rsn::AkmSuite::from_octets([
                    0x00, 0x50, 0xf2, 2
                ])
            ),
            None
        );
    }

    #[test]
    fn mic_verification_accepts_only_the_finalized_value() {
        let kck = Kck::copy_from(&[7; 16]);
        for akm in [Akm::Psk, Akm::PskSha256, Akm::Sae] {
            let engine = &mut SoftwareAes128;
            let Ok(value) = akm.mic(&kck, engine, [b"fr".as_slice(), b"ame"]);
            assert_eq!(
                akm.verify_mic(&kck, engine, [b"frame".as_slice()], &value),
                Ok(())
            );
            assert_eq!(
                akm.verify_mic(&kck, engine, [b"framf".as_slice()], &value),
                Err(crate::MicFailure::Mismatch)
            );
        }
    }

    #[test]
    fn an_aes_cmac_mic_returns_the_engines_refusal_and_hmac_needs_no_engine() {
        let kck = Kck::copy_from(&[7; 16]);
        for akm in [Akm::PskSha256, Akm::Sae] {
            assert_eq!(
                akm.mic(&kck, &mut Refusing, [b"frame".as_slice()]).err(),
                Some(oer_portable_crypto::block::EngineStopped)
            );
            assert_eq!(
                akm.verify_mic(&kck, &mut Refusing, [b"frame".as_slice()], &[0; 16]),
                Err(crate::MicFailure::Engine(
                    oer_portable_crypto::block::EngineStopped
                ))
            );
        }
        assert!(
            Akm::Psk
                .mic(&kck, &mut Refusing, [b"frame".as_slice()])
                .is_ok()
        );
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
        assert_eq!(
            expanded(Akm::PskSha256, &pmk, &context).as_flattened(),
            expected
        );
        assert_eq!(Akm::PskSha256.key_descriptor_version().get(), 3);

        // Computed independently with Python's cryptography CMAC.
        let mut engine = SoftwareAes128;
        let Ok(mic) = Akm::PskSha256.mic(
            &Kck::copy_from(&[7; 16]),
            &mut engine,
            [b"eapol-frame-bytes".as_slice()],
        );
        assert_eq!(
            mic,
            [
                0xc9, 0x54, 0x6e, 0xe7, 0xb6, 0xed, 0x61, 0x8c, 0x15, 0x91, 0x63, 0xcb, 0x3a, 0x83,
                0xe9, 0xb4
            ]
        );
    }

    #[test]
    fn sae_keys_like_psk_sha256_under_the_akm_defined_descriptor() {
        assert_eq!(
            Akm::from_suite_selector(
                oer_protocols_ieee80211_wire::security::rsn::AkmSuite::from_octets([
                    0x00, 0x0f, 0xac, 8
                ])
            ),
            Some(Akm::Sae)
        );
        assert_eq!(
            Akm::Sae.suite_selector(),
            oer_protocols_ieee80211_wire::security::rsn::AkmSuite::from_octets([
                0x00, 0x0f, 0xac, 8
            ])
        );
        assert_eq!(Akm::Sae.key_descriptor_version().get(), 0);
        let pmk: [u8; 32] = core::array::from_fn(|index| index as u8);
        let context: [u8; 76] = core::array::from_fn(|index| 100 + index as u8);
        assert_eq!(
            expanded(Akm::Sae, &pmk, &context),
            expanded(Akm::PskSha256, &pmk, &context)
        );
        let engine = &mut SoftwareAes128;
        let kck = Kck::copy_from(&[7; 16]);
        let message = [b"eapol-frame-bytes".as_slice()];
        assert_eq!(
            Akm::Sae.mic(&kck, engine, message),
            Akm::PskSha256.mic(&kck, engine, message)
        );
    }
}
