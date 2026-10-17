#![no_std]
#![forbid(unsafe_code)]

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

pub mod aes;
pub mod akm;
pub mod bip;
mod ccm;
pub mod ccmp_mpdu;
pub mod element;
pub mod frames;
mod kdf;
pub mod keys;
pub mod management_ccmp;
pub mod retry;
pub mod runner;
pub mod sae;
pub mod state;
pub mod supplicant;

pub mod crypto;
pub mod eapol;

pub(crate) use akm::AkmKeys;
pub use akm::RSN_MIC_LEN;
pub use crypto::{
    AssociationSecurityBinding, PSK_PASSPHRASE_MAX_LEN, PSK_PASSPHRASE_MIN_LEN,
    PSK_PBKDF2_ITERATIONS, PSK_SSID_MAX_LEN, Pmk, PskDerivationError, Ptk, PtkContext, RSN_KCK_LEN,
    RSN_KEK_LEN, RSN_KEY_DATA_CAPACITY, RSN_PTK_LEN, RSN_UNWRAPPED_KEY_DATA_CAPACITY,
};
pub(crate) use crypto::{RsnKeyConfirmationKey, RsnKeyEncryptionKey};
pub use eapol::{
    DEFAULT_EAPOL_FRAME_CAPACITY, EAPOL_HEADER_LEN, EAPOL_KEY_FIXED_LEN, EAPOL_KEY_PACKET_LEN,
    EAPOL_PACKET_TYPE_KEY, EapolCopyError, EapolKeyFrame, EapolKeyInfo, EapolKeyMessage,
    EapolParseError, OwnedEapolFrame, RSN_KEY_DESCRIPTOR_TYPE,
};
pub(crate) use oer_ieee80211_mac::security::rsn::Akm;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnInterface {
    Station,
    AccessPoint,
}
