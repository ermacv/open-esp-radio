//! Zeroizing group-key owners, key-data construction and parsing.

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
            .checked_add(24 + igtk_len)
            .ok_or(RsnFrameError::CapacityExceeded)?;
        let padding = if unpadded_len < 16 {
            16 - unpadded_len
        } else {
            (8 - unpadded_len % 8) % 8
        };
        let len = unpadded_len
            .checked_add(padding)
            .ok_or(RsnFrameError::CapacityExceeded)?;
        if len > N {
            return Err(RsnFrameError::CapacityExceeded);
        }

        let mut bytes = [0; N];
        let elements_end = authenticator_elements.len();
        bytes[..elements_end].copy_from_slice(authenticator_elements);
        let kde = &mut bytes[elements_end..elements_end + 24];
        kde[0] = VENDOR_ELEMENT_ID;
        kde[1] = 22;
        kde[2..5].copy_from_slice(&RSN_OUI);
        kde[5] = GTK_KDE_TYPE;
        kde[6] = gtk.key_id.get() | u8::from(gtk.transmit) << 2;
        kde[7] = 0;
        kde[8..24].copy_from_slice(gtk.key());
        if let Some(igtk) = igtk {
            let kde = &mut bytes[elements_end + 24..unpadded_len];
            kde[0] = VENDOR_ELEMENT_ID;
            kde[1] = (IGTK_KDE_LEN - 2) as u8;
            kde[2..5].copy_from_slice(&RSN_OUI);
            kde[5] = IGTK_KDE_TYPE;
            kde[6..8].copy_from_slice(&u16::from(igtk.key_id()).to_le_bytes());
            kde[8..8 + RSN_IPN_LEN].copy_from_slice(&igtk.packet_number());
            kde[8 + RSN_IPN_LEN..].copy_from_slice(igtk.key());
        }
        if padding != 0 {
            bytes[unpadded_len] = VENDOR_ELEMENT_ID;
        }
        Ok(Self { len, bytes })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
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
    let mut offset = 0;
    let mut saw_rsn = false;
    let mut saw_rsnxe = false;
    let mut gtk = None;
    let mut igtk = None;

    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        if remaining.iter().all(|byte| *byte == 0)
            || (remaining[0] == VENDOR_ELEMENT_ID && remaining[1..].iter().all(|byte| *byte == 0))
        {
            break;
        }
        if remaining.len() < 2 {
            return Err(RsnFrameError::MalformedKeyData);
        }
        let element_len = remaining[1] as usize + 2;
        if element_len > remaining.len() {
            return Err(RsnFrameError::MalformedKeyData);
        }
        let element = &remaining[..element_len];
        match (element[0], &context) {
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
            (VENDOR_ELEMENT_ID, _) if element.len() >= 6 && element[2..5] == RSN_OUI => {
                match element[5] {
                    GTK_KDE_TYPE => {
                        if element.len() != 24 || element[7] != 0 || element[6] & !0x07 != 0 {
                            return Err(RsnFrameError::UnsupportedKeyData);
                        }
                        if gtk.is_some() {
                            return Err(RsnFrameError::DuplicateGtk);
                        }
                        let key = element[8..24]
                            .try_into()
                            .map_err(|_| RsnFrameError::MalformedKeyData)?;
                        let key_id = GroupKeyId::new(element[6] & 0x03)
                            .ok_or(RsnFrameError::InvalidKeyId)?;
                        gtk = Some(RsnGtk::new(key_id, element[6] & 0x04 != 0, key));
                    }
                    IGTK_KDE_TYPE => {
                        if !management_protection {
                            return Err(RsnFrameError::UnexpectedIgtk);
                        }
                        if element.len() != IGTK_KDE_LEN {
                            return Err(RsnFrameError::UnsupportedKeyData);
                        }
                        if igtk.is_some() {
                            return Err(RsnFrameError::DuplicateIgtk);
                        }
                        let key_id = u16::from_le_bytes([element[6], element[7]]);
                        let mut packet_number = [0; RSN_IPN_LEN];
                        packet_number.copy_from_slice(&element[8..14]);
                        let key = element[14..30]
                            .try_into()
                            .map_err(|_| RsnFrameError::MalformedKeyData)?;
                        let key_id =
                            u8::try_from(key_id).map_err(|_| RsnFrameError::InvalidKeyId)?;
                        igtk = Some(RsnIgtk::new(key_id, packet_number, key)?);
                    }
                    _ => return Err(RsnFrameError::UnsupportedKeyData),
                }
            }
            _ => return Err(RsnFrameError::UnsupportedKeyData),
        }
        offset += element_len;
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
