//! CCMP-128 CCM: eight-byte MIC, thirteen-byte nonce, two-byte body length,
//! over the caller's AES-128 engine (`oer_crypto::ccm`). The temporal key is
//! loaded for one frame and erased from the engine when the frame is done.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
use oer_crypto::{Aes128Key, block::Aes128Engine, ccm::CcmError};
use oer_ieee80211_wire::ccmp::protected::CcmpAad as Aad;

pub(crate) const MIC_LEN: usize = 8;
pub(crate) const NONCE_LEN: usize = oer_crypto::ccm::NONCE_LEN;

pub(crate) fn seal<E: Aes128Engine>(
    engine: &mut E,
    key: &[u8; 16],
    nonce: &[u8; NONCE_LEN],
    aad: &Aad,
    body: &mut [u8],
) -> Result<[u8; MIC_LEN], CcmError> {
    let key = Aes128Key::copy_from(key);
    let mut keyed = engine.load(&key).map_err(CcmError::Engine)?;
    drop(key);
    oer_crypto::ccm::seal(&mut keyed, nonce, aad.as_bytes(), body)
}

pub(crate) fn open<E: Aes128Engine>(
    engine: &mut E,
    key: &[u8; 16],
    nonce: &[u8; NONCE_LEN],
    aad: &Aad,
    body: &mut [u8],
    mic: &[u8; MIC_LEN],
) -> Result<(), CcmError> {
    let key = Aes128Key::copy_from(key);
    let mut keyed = engine.load(&key).map_err(CcmError::Engine)?;
    drop(key);
    oer_crypto::ccm::open(&mut keyed, nonce, aad.as_bytes(), body, mic)
}
