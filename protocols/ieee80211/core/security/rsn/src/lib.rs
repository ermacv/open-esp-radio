#![cfg_attr(
    test,
    expect(
        clippy::cast_possible_truncation,
        reason = "test code: a cast or a dropped result in a test loses no product's value"
    )
)]
#![cfg_attr(
    test,
    expect(
        clippy::cast_sign_loss,
        reason = "test code: a cast or a dropped result in a test loses no product's value"
    )
)]
#![cfg_attr(
    test,
    expect(
        clippy::let_underscore_must_use,
        reason = "test code: a cast or a dropped result in a test loses no product's value"
    )
)]
#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

//! Allocation-free RSN (IEEE 802.11 robust security network) protocol
//! primitives shared by WPA2 and WPA3 authentication suites.
//!
//! The crate validates and classifies complete EAPOL-Key packets, owns the
//! station/authenticator four-way-handshake state machines and joins station
//! PTK/MIC/key-data processing to typed key-install requests. The negotiated
//! [`Akm`] selects the key descriptor version, pairwise key expansion and
//! EAPOL-Key MIC; everything else is shared. PSK (`00-0F-AC:2`) and
//! PSK-SHA256 (`00-0F-AC:6`) are implemented. With management frame
//! protection negotiated, group keys carry the IGTK and [`bip`] verifies
//! group-addressed robust management frames. Platform MAC crates remain
//! responsible only for executing key-install and transmit requests.

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod test_engine;

/// Unwrap `wrapped` under `kek` in the software engine: the plaintext.
#[cfg(test)]
pub(crate) fn test_unwrap_bytes(
    kek: &Kek,
    wrapped: &[u8],
) -> Result<
    std::vec::Vec<u8>,
    oer_portable_crypto::rfc3394::KeyUnwrapError<core::convert::Infallible>,
> {
    let mut buffer = oer_protocols_ieee80211_core_security_cipher::aes::RsnKeyData::new();
    let wrapped = buffer
        .wrapped(wrapped)
        .map_err(|_| oer_portable_crypto::rfc3394::KeyUnwrapError::InvalidLength)?;
    oer_portable_crypto::rfc3394::unwrap(
        &mut oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128,
        kek.aes_key(),
        wrapped,
    )
    .map(|plaintext| plaintext.to_vec())
}

pub mod akm;
pub mod authenticator;
pub mod bip;
pub mod element;
pub mod frames;
mod kdf;
pub mod keys;
mod mac;
pub mod management_ccmp;
pub mod nonce;
pub mod pmksa;
pub mod retry;
pub mod sae;
pub mod state;
pub mod supplicant;

pub mod crypto;
pub mod eapol;

pub(crate) use akm::AkmKeys;
pub use akm::RSN_MIC_LEN;
pub(crate) use crypto::RsnKeyConfirmationKey;
pub use crypto::{
    AssociationSecurityBinding, Kck, Kek, PSK_PASSPHRASE_MAX_LEN, PSK_PASSPHRASE_MIN_LEN,
    PSK_PBKDF2_ITERATIONS, PSK_SSID_MAX_LEN, Pmk, PmkDerivation, PskDerivationError, Ptk,
    PtkContext, RSN_KCK_LEN, RSN_KEK_LEN, RSN_PTK_LEN,
};
pub use eapol::{
    DEFAULT_EAPOL_FRAME_CAPACITY, EapolCopyError, EapolKeyFrame, EapolKeyMessage, MicFailure,
    OwnedEapolFrame,
};
pub(crate) use oer_protocols_ieee80211_wire::security::rsn::Akm;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnInterface {
    Station,
    AccessPoint,
}
