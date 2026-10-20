//! CCMP-128 CCM: eight-byte MIC, thirteen-byte nonce, two-byte body length.
//! RustCrypto authenticates and encrypts in place with a detached MIC.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
use crate::ccmp_mpdu::Aad;
use aes::Aes128;
use ccm::{
    Ccm, Nonce, Tag,
    aead::{AeadInPlace, KeyInit},
    consts::{U8, U13},
};

type CcmpCcm = Ccm<Aes128, U8, U13>;
pub(crate) const MIC_LEN: usize = 8;
pub(crate) const NONCE_LEN: usize = 13;

pub(crate) fn seal(
    key: &[u8; 16],
    nonce: &[u8; NONCE_LEN],
    aad: &Aad,
    body: &mut [u8],
) -> Result<[u8; MIC_LEN], ccm::Error> {
    // Both arrays have their cipher's exact, statically checked lengths.
    let cipher = CcmpCcm::new(key.into());
    cipher
        .encrypt_in_place_detached(Nonce::<U13>::from_slice(nonce), aad.bytes(), body)
        .map(Into::into)
}

pub(crate) fn open(
    key: &[u8; 16],
    nonce: &[u8; NONCE_LEN],
    aad: &Aad,
    body: &mut [u8],
    mic: &[u8; MIC_LEN],
) -> Result<(), ccm::Error> {
    let cipher = CcmpCcm::new(key.into());
    // RustCrypto checks the detached tag with ConstantTimeEq. Aes128's
    // enabled zeroize feature erases its schedule when the cipher is dropped.
    cipher.decrypt_in_place_detached(
        Nonce::<U13>::from_slice(nonce),
        aad.bytes(),
        body,
        Tag::<U8>::from_slice(mic),
    )
}
