//! Zeroizing group-key owners, key-data construction and parsing.
//!
//! Received bytes reach this module; nothing here panics on them.

use super::*;
use oer_portable_crypto::Aes128Key;
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_wire::bip::IgtkKeyId;
use oer_protocols_ieee80211_wire::ccmp::GroupKeyId;
use oer_protocols_ieee80211_wire::eapol::{
    GTK_KDE_LEN, GtkKde, GtkKdeInfo, GtkTx, IGTK_KDE_LEN, IgtkKde, Kde, KeyDataElements,
    key_data_padding, write_key_data_padding,
};
use oer_protocols_ieee80211_wire::element::ElementId;
use oer_protocols_ieee80211_wire::security::GroupManagementCipher;

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct RsnGtk {
    #[zeroize(skip)]
    key_id: GroupKeyId,
    #[zeroize(skip)]
    tx: GtkTx,
    key: Ccmp128Key,
}

impl RsnGtk {
    /// Copy borrowed key data straight into its zeroizing protocol owner.
    pub const fn new(key_id: GroupKeyId, tx: GtkTx, key: &[u8; RSN_GTK_LEN]) -> Self {
        Self {
            key_id,
            tx,
            key: Ccmp128Key::new(key),
        }
    }

    pub const fn key_id(&self) -> GroupKeyId {
        self.key_id
    }

    /// The GTK KDE's Tx bit.
    pub const fn tx(&self) -> GtkTx {
        self.tx
    }

    /// The GTK, a CCMP-128 temporal key, lent.
    pub const fn key(&self) -> &Ccmp128Key {
        &self.key
    }

    /// A second owner of the GTK, for its installation.
    pub const fn install_key(&self) -> Ccmp128Key {
        Ccmp128Key::new(self.key.as_bytes())
    }
}

/// The integrity group temporal key (IGTK, IEEE Std 802.11-2020, 12.7.1.5):
/// the BIP-CMAC-128 key of group-addressed management frames alone.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Igtk(Aes128Key);

impl Igtk {
    const fn copy_from(bytes: &[u8; RSN_IGTK_LEN]) -> Self {
        Self(Aes128Key::copy_from(bytes))
    }

    /// Lend the key's octets, for its IGTK KDE.
    pub const fn as_bytes(&self) -> &[u8; RSN_IGTK_LEN] {
        self.0.as_bytes()
    }

    /// Lend the key to an AES-128 engine.
    pub const fn aes_key(&self) -> &Aes128Key {
        &self.0
    }

    /// A second zeroizing owner of the same key, for a BIP owner.
    pub(crate) const fn second_owner(&self) -> Self {
        Self::copy_from(self.0.as_bytes())
    }

    /// Whether `other` holds the same key.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

/// Integrity group temporal key of protected management frames, from an
/// IGTK KDE: its key identifier (4 or 5), the IGTK packet number the next
/// BIP frame must exceed, and the BIP-CMAC-128 key.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct RsnIgtk {
    /// Public: only the key material needs erasure.
    #[zeroize(skip)]
    key_id: IgtkKeyId,
    packet_number: [u8; RSN_IPN_LEN],
    key: Igtk,
}

impl RsnIgtk {
    pub const fn new(
        key_id: IgtkKeyId,
        packet_number: [u8; RSN_IPN_LEN],
        key: &[u8; RSN_IGTK_LEN],
    ) -> Self {
        Self {
            key_id,
            packet_number,
            key: Igtk::copy_from(key),
        }
    }

    /// The same IGTK announced with another IPN: a second owner of its key.
    pub const fn with_packet_number(&self, packet_number: [u8; RSN_IPN_LEN]) -> Self {
        Self {
            key_id: self.key_id,
            packet_number,
            key: self.key.second_owner(),
        }
    }

    /// Announce this IGTK with another IPN from now on.
    pub const fn set_packet_number(&mut self, packet_number: [u8; RSN_IPN_LEN]) {
        self.packet_number = packet_number;
    }

    pub const fn key_id(&self) -> IgtkKeyId {
        self.key_id
    }

    /// The IPN, as its little-endian six bytes.
    pub const fn packet_number(&self) -> [u8; RSN_IPN_LEN] {
        self.packet_number
    }

    pub const fn key(&self) -> &Igtk {
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
    bytes: heapless::Vec<u8, N>,
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
        let padding = key_data_padding(unpadded_len).ok_or(RsnFrameError::CapacityExceeded)?;
        let len = unpadded_len
            .checked_add(padding)
            .ok_or(RsnFrameError::CapacityExceeded)?;
        let mut bytes = heapless::Vec::new();
        bytes
            .resize(len, 0)
            .map_err(|_| RsnFrameError::CapacityExceeded)?;
        let (elements, rest) = bytes
            .as_mut_slice()
            .split_at_mut_checked(authenticator_elements.len())
            .ok_or(RsnFrameError::CapacityExceeded)?;
        for (target, source) in elements.iter_mut().zip(authenticator_elements) {
            *target = *source;
        }
        let (gtk_kde, rest) = rest
            .split_first_chunk_mut::<GTK_KDE_LEN>()
            .ok_or(RsnFrameError::CapacityExceeded)?;
        *gtk_kde = GtkKde {
            info: GtkKdeInfo::new(gtk.key_id).with_tx(gtk.tx),
            key: gtk.key().as_bytes(),
        }
        .encode();
        let padding_bytes = match igtk {
            Some(igtk) => {
                let (igtk_kde, rest) = rest
                    .split_first_chunk_mut::<IGTK_KDE_LEN>()
                    .ok_or(RsnFrameError::CapacityExceeded)?;
                *igtk_kde = IgtkKde {
                    key_id: igtk.key_id().field(),
                    packet_number: igtk.packet_number(),
                    key: igtk.key().as_bytes(),
                }
                .encode();
                rest
            }
            None => rest,
        };
        write_key_data_padding(padding_bytes);
        Ok(Self { bytes })
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.as_slice()
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

/// Parse the decrypted key-data body of pairwise Message 3. With a
/// negotiated `group_management` cipher, the IGTK KDE is required; without
/// one, refused.
pub fn parse_gtk_key_data(
    bytes: &[u8],
    expected_rsn_ie: &[u8],
    expected_rsnxe: &[u8],
    group_management: Option<GroupManagementCipher>,
) -> Result<RsnGroupKeys, RsnFrameError> {
    parse_key_data(
        bytes,
        KeyDataContext::Message3 {
            expected_rsn_ie,
            expected_rsnxe,
        },
        group_management,
    )
}

/// Parse the encrypted key-data body of a connected-state Group Message 1.
///
/// Unlike pairwise Message 3, a group rekey carries its group-key KDEs
/// without a copy of the association RSN IEs. Only those KDEs and key-wrap
/// padding are accepted; unrelated elements fail closed.
pub fn parse_group_gtk_key_data(
    bytes: &[u8],
    group_management: Option<GroupManagementCipher>,
) -> Result<RsnGroupKeys, RsnFrameError> {
    parse_key_data(bytes, KeyDataContext::GroupMessage1, group_management)
}

fn parse_key_data(
    bytes: &[u8],
    context: KeyDataContext<'_>,
    group_management: Option<GroupManagementCipher>,
) -> Result<RsnGroupKeys, RsnFrameError> {
    let mut saw_rsn = false;
    let mut saw_rsnxe = false;
    let mut gtk = None;
    let mut igtk = None;

    for element in KeyDataElements::new(bytes) {
        let element = element.map_err(|_| RsnFrameError::MalformedKeyData)?;
        let Some(&first) = element.first() else {
            return Err(RsnFrameError::MalformedKeyData);
        };
        match (ElementId::new(first), &context) {
            (
                ElementId::RSN,
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
            (ElementId::RSNX, KeyDataContext::Message3 { expected_rsnxe, .. }) => {
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
            (ElementId::VENDOR_SPECIFIC, _) => {
                let Some(Kde {
                    data_type,
                    data: body,
                }) = Kde::parse(element)
                else {
                    return Err(RsnFrameError::UnsupportedKeyData);
                };
                match data_type {
                    KdeDataType::GTK => {
                        let GtkKde { info, key } =
                            GtkKde::parse(body).ok_or(RsnFrameError::UnsupportedKeyData)?;
                        if gtk.is_some() {
                            return Err(RsnFrameError::DuplicateGtk);
                        }
                        let key_id = info.group_key_id().ok_or(RsnFrameError::InvalidKeyId)?;
                        gtk = Some(RsnGtk::new(key_id, info.tx(), key));
                    }
                    KdeDataType::IGTK => {
                        if group_management.is_none() {
                            return Err(RsnFrameError::UnexpectedIgtk);
                        }
                        let IgtkKde {
                            key_id,
                            packet_number,
                            key,
                        } = IgtkKde::parse(body).ok_or(RsnFrameError::UnsupportedKeyData)?;
                        if igtk.is_some() {
                            return Err(RsnFrameError::DuplicateIgtk);
                        }
                        let key_id =
                            IgtkKeyId::from_field(key_id).ok_or(RsnFrameError::InvalidKeyId)?;
                        igtk = Some(RsnIgtk::new(key_id, packet_number, key));
                    }
                    _ => return Err(RsnFrameError::UnsupportedKeyData),
                }
            }
            _ => return Err(RsnFrameError::UnsupportedKeyData),
        }
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
    if group_management.is_some() && igtk.is_none() {
        return Err(RsnFrameError::MissingIgtk);
    }
    Ok(RsnGroupKeys { gtk, igtk })
}
