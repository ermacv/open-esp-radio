//! Fixed WPA2-CCMP EAPOL frame construction and GTK key-data parsing.

use oer_ieee80211_wire::{
    eapol::{GTK_KDE_TYPE, IGTK_KDE_TYPE, KDE_ELEMENT_ID, KeyInformation},
    ethertype::ETHER_TYPE_EAPOL,
    security::rsn::{RSN_ELEMENT_ID, RSNXE_ELEMENT_ID},
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::state::{RsnApState, RsnStaState, RsnTransmit, RsnTxMessage};
use crate::{
    Akm, EAPOL_KEY_FIXED_LEN, EAPOL_KEY_PACKET_LEN, EAPOL_PACKET_TYPE_KEY, EapolKeyFrame, Ptk,
    RSN_KCK_LEN, RSN_KEY_DESCRIPTOR_TYPE, RSN_MIC_LEN, RsnInterface, RsnKeyConfirmationKey,
};

pub const RSN_IE_CAPACITY: usize = 64;
pub const RSN_ASSOC_SECURITY_IES_CAPACITY: usize = 128;
pub const RSN_GTK_LEN: usize = 16;
/// BIP-CMAC-128 key length.
pub const RSN_IGTK_LEN: usize = 16;
/// IGTK packet number length.
pub const RSN_IPN_LEN: usize = 6;
pub const RSN_PLAIN_KEY_DATA_CAPACITY: usize = 128;
pub const RSN_TX_EAPOL_CAPACITY: usize = 512;
pub const RSN_TX_ETHERNET_CAPACITY: usize = RSN_TX_EAPOL_CAPACITY + 14;

/// IGTK KDE: key id (2), IPN (6) and IGTK (16) after the OUI and type.
const IGTK_KDE_LEN: usize = 2 + 4 + 2 + RSN_IPN_LEN + RSN_IGTK_LEN;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnFrameError {
    CapacityExceeded,
    InvalidRsnIe,
    InvalidRsnxe,
    InvalidKeyId,
    ZeroNonce,
    EmptyKeyData,
    MalformedKeyData,
    UnsupportedKeyData,
    DuplicateRsnIe,
    DuplicateRsnxe,
    DuplicateGtk,
    DuplicateIgtk,
    MissingRsnIe,
    MissingRsnxe,
    UnexpectedRsnxe,
    MissingGtk,
    /// Management frame protection is negotiated but no IGTK KDE came.
    MissingIgtk,
    /// An IGTK KDE came although management frame protection is not
    /// negotiated.
    UnexpectedIgtk,
    RsnIeMismatch,
    RsnxeMismatch,
    UnexpectedTransmitAction,
}

mod key_data;
mod security_ies;
mod transmit;

pub use key_data::{
    RsnGroupKeys, RsnGtk, RsnIgtk, RsnPlainKeyData, parse_group_gtk_key_data, parse_gtk_key_data,
};
pub use security_ies::{OwnedAssociationSecurityIes, OwnedRsnIe};
pub use transmit::{RsnEthernetFrame, RsnTxFrame, build_ap_action_frame, build_sta_action_frame};

#[cfg(test)]
pub(crate) mod captured_m3;
#[cfg(test)]
mod tests;
