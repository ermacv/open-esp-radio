//! Zeroizing group-key owners, key-data construction and parsing.
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use super::*;
use oer_ieee80211_mac::ccmp::GroupKeyId;

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct RsnGtk {
    #[zeroize(skip)]
    key_id: GroupKeyId,
    transmit: bool,
    key: [u8; RSN_GTK_LEN],
}

impl RsnGtk {
    /// Copy borrowed key data straight into its zeroizing protocol owner.
    pub fn new(key_id: GroupKeyId, transmit: bool, key: &[u8; RSN_GTK_LEN]) -> Self {
        let mut gtk = Self {
            key_id,
            transmit,
            key: [0; RSN_GTK_LEN],
        };
        gtk.key.copy_from_slice(key);
        gtk
    }

    pub const fn key_id(&self) -> GroupKeyId {
        self.key_id
    }

    pub const fn transmit(&self) -> bool {
        self.transmit
    }

    pub const fn key(&self) -> &[u8; RSN_GTK_LEN] {
        &self.key
    }
}

/// Integrity group temporal key of protected management frames, from an
/// IGTK KDE: its key identifier (4 or 5), the IGTK packet number the next
/// BIP frame must exceed, and the BIP-CMAC-128 key.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct RsnIgtk {
    key_id: u8,
    packet_number: [u8; RSN_IPN_LEN],
    key: [u8; RSN_IGTK_LEN],
}

impl RsnIgtk {
    pub fn new(
        key_id: u8,
        packet_number: [u8; RSN_IPN_LEN],
        key: &[u8; RSN_IGTK_LEN],
    ) -> Result<Self, RsnFrameError> {
        if !(4..=5).contains(&key_id) {
            return Err(RsnFrameError::InvalidKeyId);
        }
        let mut igtk = Self {
            key_id,
            packet_number,
            key: [0; RSN_IGTK_LEN],
        };
        igtk.key.copy_from_slice(key);
        Ok(igtk)
    }

    pub const fn key_id(&self) -> u8 {
        self.key_id
    }

    /// The IPN, as its little-endian six bytes.
    pub const fn packet_number(&self) -> [u8; RSN_IPN_LEN] {
        self.packet_number
    }

    pub const fn key(&self) -> &[u8; RSN_IGTK_LEN] {
        &self.key
    }
}

/// The group keys one key-data body delivers: the GTK, and the IGTK when
/// the association protects its management frames.
pub struct RsnGroupKeys {
    pub gtk: RsnGtk,
    pub igtk: Option<RsnIgtk>,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct RsnPlainKeyData<const N: usize = RSN_PLAIN_KEY_DATA_CAPACITY> {
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> RsnPlainKeyData<N> {
    /// Message 3's key data: the authenticator's RSN element and RSNXE
    /// exactly as it advertises them, the GTK KDE and, when management
    /// frames are protected, the IGTK KDE, padded as IEEE 802.11 12.7.2
    /// requires.
    pub fn build(
        authenticator_elements: &[u8],
        gtk: &RsnGtk,
        igtk: Option<&RsnIgtk>,
    ) -> Result<Self, RsnFrameError> {
        let igtk_len = if igtk.is_some() { IGTK_KDE_LEN } else { 0 };
        let unpadded_len = authenticator_elements
            .len()
            .checked_add(GTK_KDE_LEN)
            .and_then(|len| len.checked_add(igtk_len))
            .ok_or(RsnFrameError::CapacityExceeded)?;
        let padding = if unpadded_len < 16 {
            16_usize.saturating_sub(unpadded_len)
        } else {
            unpadded_len.wrapping_neg() % 8
        };
        let len = unpadded_len
            .checked_add(padding)
            .ok_or(RsnFrameError::CapacityExceeded)?;
        let mut bytes = [0; N];
        let (elements, rest) = bytes
            .get_mut(..len)
            .ok_or(RsnFrameError::CapacityExceeded)?
            .split_at_mut_checked(authenticator_elements.len())
            .ok_or(RsnFrameError::CapacityExceeded)?;
        elements.copy_from_slice(authenticator_elements);
        let (gtk_kde, rest) = rest
            .split_first_chunk_mut::<GTK_KDE_LEN>()
            .ok_or(RsnFrameError::CapacityExceeded)?;
        *gtk_kde = gtk_kde_bytes(gtk);
        let padding_bytes = match igtk {
            Some(igtk) => {
                let (igtk_kde, rest) = rest
                    .split_first_chunk_mut::<IGTK_KDE_LEN>()
                    .ok_or(RsnFrameError::CapacityExceeded)?;
                *igtk_kde = igtk_kde_bytes(igtk);
                rest
            }
            None => rest,
        };
        if let Some(first) = padding_bytes.first_mut() {
            *first = VENDOR_ELEMENT_ID;
        }
        Ok(Self { len, bytes })
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or_default()
    }
}

const GTK_KDE_LEN: usize = 24;

fn gtk_kde_bytes(gtk: &RsnGtk) -> [u8; GTK_KDE_LEN] {
    let [o0, o1, o2] = RSN_OUI;
    let mut kde = [0; GTK_KDE_LEN];
    let (header, key) = kde.split_at_mut(8);
    header.copy_from_slice(&[
        VENDOR_ELEMENT_ID,
        (GTK_KDE_LEN - 2) as u8,
        o0,
        o1,
        o2,
        GTK_KDE_TYPE,
        gtk.key_id.get() | u8::from(gtk.transmit) << 2,
        0,
    ]);
    key.copy_from_slice(gtk.key());
    kde
}

fn igtk_kde_bytes(igtk: &RsnIgtk) -> [u8; IGTK_KDE_LEN] {
    let [o0, o1, o2] = RSN_OUI;
    let [id0, id1] = u16::from(igtk.key_id()).to_le_bytes();
    let mut kde = [0; IGTK_KDE_LEN];
    let (header, rest) = kde.split_at_mut(8);
    header.copy_from_slice(&[
        VENDOR_ELEMENT_ID,
        (IGTK_KDE_LEN - 2) as u8,
        o0,
        o1,
        o2,
        IGTK_KDE_TYPE,
        id0,
        id1,
    ]);
    let (packet_number, key) = rest.split_at_mut(RSN_IPN_LEN);
    packet_number.copy_from_slice(&igtk.packet_number());
    key.copy_from_slice(igtk.key());
    kde
}

/// Which elements beside the group-key KDEs a key-data body carries.
enum KeyDataContext<'a> {
    /// Pairwise Message 3: a copy of the association RSN IE and RSNXE.
    Message3 {
        expected_rsn_ie: &'a [u8],
        expected_rsnxe: &'a [u8],
    },
    /// Group Message 1: only the group-key KDEs.
    GroupMessage1,
}

/// Parse the decrypted key-data body of pairwise Message 3. With
/// `management_protection`, the IGTK KDE is required; without it, refused.
pub fn parse_gtk_key_data(
    bytes: &[u8],
    expected_rsn_ie: &[u8],
    expected_rsnxe: &[u8],
    management_protection: bool,
) -> Result<RsnGroupKeys, RsnFrameError> {
    parse_key_data(
        bytes,
        KeyDataContext::Message3 {
            expected_rsn_ie,
            expected_rsnxe,
        },
        management_protection,
    )
}

/// Parse the encrypted key-data body of a connected-state Group Message 1.
///
/// Unlike pairwise Message 3, a group rekey carries its group-key KDEs
/// without a copy of the association RSN IEs. Only those KDEs and key-wrap
/// padding are accepted; unrelated elements fail closed.
pub fn parse_group_gtk_key_data(
    bytes: &[u8],
    management_protection: bool,
) -> Result<RsnGroupKeys, RsnFrameError> {
    parse_key_data(bytes, KeyDataContext::GroupMessage1, management_protection)
}

fn parse_key_data(
    bytes: &[u8],
    context: KeyDataContext<'_>,
    management_protection: bool,
) -> Result<RsnGroupKeys, RsnFrameError> {
    let mut remaining = bytes;
    let mut saw_rsn = false;
    let mut saw_rsnxe = false;
    let mut gtk = None;
    let mut igtk = None;

    while let Some((&first, after_first)) = remaining.split_first() {
        if remaining.iter().all(|byte| *byte == 0)
            || (first == VENDOR_ELEMENT_ID && after_first.iter().all(|byte| *byte == 0))
        {
            break;
        }
        let Some(&[_, length]) = remaining.first_chunk::<2>() else {
            return Err(RsnFrameError::MalformedKeyData);
        };
        let Some((element, rest)) = remaining.split_at_checked(usize::from(length) + 2) else {
            return Err(RsnFrameError::MalformedKeyData);
        };
        match (first, &context) {
            (
                RSN_ELEMENT_ID,
                KeyDataContext::Message3 {
                    expected_rsn_ie, ..
                },
            ) => {
                if saw_rsn {
                    return Err(RsnFrameError::DuplicateRsnIe);
                }
                if element != *expected_rsn_ie {
                    return Err(RsnFrameError::RsnIeMismatch);
                }
                saw_rsn = true;
            }
            (RSNXE_ELEMENT_ID, KeyDataContext::Message3 { expected_rsnxe, .. }) => {
                if saw_rsnxe {
                    return Err(RsnFrameError::DuplicateRsnxe);
                }
                if expected_rsnxe.is_empty() {
                    return Err(RsnFrameError::UnexpectedRsnxe);
                }
                if element != *expected_rsnxe {
                    return Err(RsnFrameError::RsnxeMismatch);
                }
                saw_rsnxe = true;
            }
            (VENDOR_ELEMENT_ID, _) => {
                let [_, _, o0, o1, o2, kind, body @ ..] = element else {
                    return Err(RsnFrameError::UnsupportedKeyData);
                };
                if [*o0, *o1, *o2] != RSN_OUI {
                    return Err(RsnFrameError::UnsupportedKeyData);
                }
                match *kind {
                    GTK_KDE_TYPE => {
                        let [info, reserved, key @ ..] = body else {
                            return Err(RsnFrameError::UnsupportedKeyData);
                        };
                        let key = <&[u8; RSN_GTK_LEN]>::try_from(key)
                            .map_err(|_| RsnFrameError::UnsupportedKeyData)?;
                        if *reserved != 0 || info & !0x07 != 0 {
                            return Err(RsnFrameError::UnsupportedKeyData);
                        }
                        if gtk.is_some() {
                            return Err(RsnFrameError::DuplicateGtk);
                        }
                        let key_id =
                            GroupKeyId::new(info & 0x03).ok_or(RsnFrameError::InvalidKeyId)?;
                        gtk = Some(RsnGtk::new(key_id, info & 0x04 != 0, key));
                    }
                    IGTK_KDE_TYPE => {
                        if !management_protection {
                            return Err(RsnFrameError::UnexpectedIgtk);
                        }
                        let Some((&[id0, id1], rest)) = body.split_first_chunk::<2>() else {
                            return Err(RsnFrameError::UnsupportedKeyData);
                        };
                        let Some((&packet_number, key)) = rest.split_first_chunk::<RSN_IPN_LEN>()
                        else {
                            return Err(RsnFrameError::UnsupportedKeyData);
                        };
                        let key = <&[u8; RSN_IGTK_LEN]>::try_from(key)
                            .map_err(|_| RsnFrameError::UnsupportedKeyData)?;
                        if igtk.is_some() {
                            return Err(RsnFrameError::DuplicateIgtk);
                        }
                        let key_id = u8::try_from(u16::from_le_bytes([id0, id1]))
                            .map_err(|_| RsnFrameError::InvalidKeyId)?;
                        igtk = Some(RsnIgtk::new(key_id, packet_number, key)?);
                    }
                    _ => return Err(RsnFrameError::UnsupportedKeyData),
                }
            }
            _ => return Err(RsnFrameError::UnsupportedKeyData),
        }
        remaining = rest;
    }

    if let KeyDataContext::Message3 { expected_rsnxe, .. } = context {
        if !saw_rsn {
            return Err(RsnFrameError::MissingRsnIe);
        }
        if !expected_rsnxe.is_empty() && !saw_rsnxe {
            return Err(RsnFrameError::MissingRsnxe);
        }
    }
    let gtk = gtk.ok_or(RsnFrameError::MissingGtk)?;
    if management_protection && igtk.is_none() {
        return Err(RsnFrameError::MissingIgtk);
    }
    Ok(RsnGroupKeys { gtk, igtk })
}
