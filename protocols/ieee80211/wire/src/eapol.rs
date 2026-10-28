//! EAPOL-Key frame fields and Key Data Encapsulations (IEEE 802.11-2020
//! 12.7.2).
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

use crate::ccmp::GroupKeyId;
use crate::security::rsn::RSN_OUI;

oer_wire::bitfield! {
    /// The Key Information field of an EAPOL-Key frame, big-endian on air.
    /// SOURCE: IEEE Std 802.11-2020, 12.7.2, Figure 12-33 (Key Information
    /// field); bits 4..6 are reserved.
    pub struct KeyInformation[2] (big) {
        pub descriptor_version, with_descriptor_version: (u8) = 0..3;
        pub pairwise, with_pairwise: bool = 3..4;
        pub install, with_install: bool = 6..7;
        pub ack, with_ack: bool = 7..8;
        pub mic, with_mic: bool = 8..9;
        pub secure, with_secure: bool = 9..10;
        pub error, with_error: bool = 10..11;
        pub request, with_request: bool = 11..12;
        pub encrypted_key_data, with_encrypted_key_data: bool = 12..13;
        pub smk_message, with_smk_message: bool = 13..14;
    }
}

impl KeyInformation {
    /// Every subfield clear.
    pub const EMPTY: Self = Self::from_bytes([0; 2]);

    /// The field whose big-endian value is `raw`.
    pub const fn from_raw(raw: u16) -> Self {
        Self::from_bytes(raw.to_be_bytes())
    }

    /// The field's big-endian value.
    pub const fn raw(self) -> u16 {
        u16::from_be_bytes(self.to_bytes())
    }
}

/// The Port Access Entity group address EAPOL frames may be sent to.
///
/// SOURCE: IEEE Std 802.1X-2020, 11.1.1, Table 11-1 (PAE group addresses:
/// the nearest non-TPMR bridge group address 01-80-C2-00-00-03).
pub const PAE_GROUP_ADDRESS: [u8; 6] = [0x01, 0x80, 0xc2, 0x00, 0x00, 0x03];

/// Element ID of a Key Data Encapsulation: the vendor-specific element ID.
///
/// SOURCE: IEEE Std 802.11-2020, 12.7.2, Figure 12-35 (KDE format).
pub const KDE_ELEMENT_ID: u8 = 0xdd;
/// Data Type of the GTK KDE.
///
/// SOURCE: IEEE Std 802.11-2020, 12.7.2, Table 12-10 (KDE selectors).
pub const GTK_KDE_TYPE: u8 = 1;
/// Data Type of the IGTK KDE.
///
/// SOURCE: IEEE Std 802.11-2020, 12.7.2, Table 12-10 (KDE selectors).
pub const IGTK_KDE_TYPE: u8 = 9;
/// Octets of a KDE before its data: Type, Length, OUI and Data Type.
pub const KDE_HEADER_LEN: usize = 6;

oer_wire::bitfield! {
    /// The first octet of a GTK KDE's data.
    /// SOURCE: IEEE Std 802.11-2020, 12.7.2, Figure 12-36 (GTK KDE format):
    /// Key ID in bits 0..2, Tx in bit 2, bits 3..8 reserved.
    pub struct GtkKdeInfo[1] (little) {
        pub key_id, with_key_id: (u8) = 0..2;
        pub transmit, with_transmit: bool = 2..3;
        pub reserved, with_reserved: u8 = 3..8;
    }
}

impl GtkKdeInfo {
    /// The info octet of a GTK named `key_id`, for transmission when
    /// `transmit` holds.
    pub const fn new(key_id: GroupKeyId, transmit: bool) -> Self {
        Self::from_bytes([0])
            .with_key_id(oer_wire::U8::<2>::from_low_bits(key_id.get()))
            .with_transmit(transmit)
    }

    /// The GTK's key ID, `None` when a reserved bit is set.
    pub const fn group_key_id(self) -> Option<GroupKeyId> {
        if self.reserved() != 0 {
            return None;
        }
        GroupKeyId::new(self.key_id().get())
    }
}

/// The header of a KDE of `data_type` whose data has `data_len` octets;
/// `None` when the KDE would not fit its one-octet Length.
pub const fn kde_header(data_type: u8, data_len: usize) -> Option<[u8; KDE_HEADER_LEN]> {
    // Length counts the OUI, the Data Type and the data.
    let Some(length) = data_len.checked_add(4) else {
        return None;
    };
    if length > u8::MAX as usize {
        return None;
    }
    let [o0, o1, o2] = RSN_OUI;
    Some([KDE_ELEMENT_ID, length as u8, o0, o1, o2, data_type])
}

/// One KDE of the IEEE 802.11 OUI: its Data Type and data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Kde<'a> {
    pub data_type: u8,
    pub data: &'a [u8],
}

impl<'a> Kde<'a> {
    /// The KDE a complete element `element` carries, `None` for another
    /// element, another OUI or an element shorter than the KDE header.
    pub fn parse(element: &'a [u8]) -> Option<Self> {
        let (&[id, _, o0, o1, o2, data_type], data) =
            element.split_first_chunk::<KDE_HEADER_LEN>()?;
        if id != KDE_ELEMENT_ID || [o0, o1, o2] != RSN_OUI {
            return None;
        }
        Some(Self { data_type, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_information_round_trips_big_endian() {
        let info = KeyInformation::EMPTY
            .with_descriptor_version(oer_wire::U8::constant::<2>())
            .with_pairwise(true)
            .with_install(true)
            .with_ack(true)
            .with_mic(true)
            .with_secure(true)
            .with_encrypted_key_data(true);
        assert_eq!(info.raw(), 0x13ca);
        assert_eq!(info.to_bytes(), [0x13, 0xca]);
        let parsed = KeyInformation::from_raw(0x13ca);
        assert_eq!(parsed.descriptor_version().get(), 2);
        assert!(parsed.pairwise() && parsed.install() && parsed.ack());
        assert!(parsed.mic() && parsed.secure() && parsed.encrypted_key_data());
        assert!(!parsed.error() && !parsed.request() && !parsed.smk_message());
    }

    #[test]
    fn gtk_info_round_trips_and_refuses_reserved_bits() {
        let info = GtkKdeInfo::new(GroupKeyId::new(2).unwrap(), true);
        assert_eq!(info.to_bytes(), [0x06]);
        assert_eq!(info.group_key_id(), GroupKeyId::new(2));
        assert!(info.transmit());
        assert_eq!(GtkKdeInfo::from_bytes([0x08]).group_key_id(), None);
    }

    #[test]
    fn a_kde_round_trips_and_a_short_or_foreign_one_is_refused() {
        let header = kde_header(GTK_KDE_TYPE, 18).unwrap();
        assert_eq!(header, [0xdd, 22, 0x00, 0x0f, 0xac, 1]);
        let mut element = [0_u8; 24];
        element[..6].copy_from_slice(&header);
        element[6] = 1;
        let kde = Kde::parse(&element).unwrap();
        assert_eq!(kde.data_type, GTK_KDE_TYPE);
        assert_eq!(kde.data.len(), 18);
        assert_eq!(Kde::parse(&element[..5]), None);
        element[2] = 0x50;
        assert_eq!(Kde::parse(&element), None);
        assert_eq!(kde_header(GTK_KDE_TYPE, 252), None);
    }
}
