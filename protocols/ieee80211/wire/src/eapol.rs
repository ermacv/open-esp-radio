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

/// Length of the EAPOL header: protocol version, packet type, body length.
///
/// SOURCE: IEEE Std 802.1X-2020, 11.3 (EAPOL PDU format).
pub const EAPOL_HEADER_LEN: usize = 4;
/// EAPOL packet type of an EAPOL-Key frame.
///
/// SOURCE: IEEE Std 802.1X-2020, 11.3.2, Table 11-3 (EAPOL Packet Types).
pub const EAPOL_PACKET_TYPE_KEY: u8 = 3;
/// Descriptor type of an IEEE 802.11 (RSN) key descriptor.
///
/// SOURCE: IEEE Std 802.11-2020, 12.7.2 (EAPOL-Key frames: Descriptor Type).
pub const RSN_KEY_DESCRIPTOR_TYPE: u8 = 2;
/// Length of the Key MIC field of the AKMs this codec carries.
///
/// SOURCE: IEEE Std 802.11-2020, 12.7.3, Table 12-8 (Key MIC length 16 for
/// AKMs 1-9).
pub const EAPOL_KEY_MIC_LEN: usize = 16;
/// The descriptor body before its Key Data: descriptor type through Key
/// Data Length.
///
/// SOURCE: IEEE Std 802.11-2020, 12.7.2, Figure 12-32 (EAPOL-Key frame).
pub const EAPOL_KEY_FIXED_LEN: usize = 95;
/// An EAPOL-Key packet without Key Data.
pub const EAPOL_KEY_PACKET_LEN: usize = EAPOL_HEADER_LEN + EAPOL_KEY_FIXED_LEN;

/// The fields before the Key MIC, after the EAPOL header: descriptor type
/// (1), Key Information (2), Key Length (2), Key Replay Counter (8), Key
/// Nonce (32), EAPOL-Key IV (16), Key RSC (8) and Key Identifier (8).
const BEFORE_MIC_LEN: usize = EAPOL_HEADER_LEN + 1 + 2 + 2 + 8 + 32 + 16 + 8 + 8;

const GEOMETRY: () = {
    if !(BEFORE_MIC_LEN + EAPOL_KEY_MIC_LEN + 2 == EAPOL_KEY_PACKET_LEN) {
        panic!("EAPOL-Key fields must cover the packet");
    }
};

/// A received or stored EAPOL-Key packet that cannot be one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapolParseError {
    /// Fewer octets than the fixed fields.
    Truncated,
    /// The body or Key Data length disagrees with the octets given.
    LengthMismatch,
    /// Another EAPOL packet type.
    NotKeyPacket,
    /// Another key descriptor type.
    NotRsnKeyDescriptor,
}

/// A checked EAPOL-Key packet over the caller's bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EapolKey<'a> {
    bytes: &'a [u8],
    protocol_version: u8,
    key_information: KeyInformation,
    key_length: [u8; 2],
    replay_counter: [u8; 8],
    nonce: &'a [u8; 32],
    key_iv: &'a [u8; 16],
    key_rsc: &'a [u8; 8],
    key_identifier: &'a [u8; 8],
    mic: &'a [u8; EAPOL_KEY_MIC_LEN],
    key_data: &'a [u8],
    before_mic: &'a [u8],
    after_mic: &'a [u8],
}

/// The zero Key MIC a MIC is computed over.
const ZERO_MIC: [u8; EAPOL_KEY_MIC_LEN] = [0; EAPOL_KEY_MIC_LEN];

impl<'a> EapolKey<'a> {
    /// Check one complete EAPOL-Key packet: its type, its descriptor and
    /// both of its lengths.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, EapolParseError> {
        let () = GEOMETRY;
        let mut reader = oer_wire::Reader::new(bytes);
        let truncated = |_| EapolParseError::Truncated;
        let [protocol_version, packet_type, length_high, length_low] =
            reader.read::<EAPOL_HEADER_LEN>().map_err(truncated)?;
        let [descriptor] = reader.read::<1>().map_err(truncated)?;
        let key_information = KeyInformation::from_bytes(reader.read::<2>().map_err(truncated)?);
        let key_length = reader.read::<2>().map_err(truncated)?;
        let replay_counter = reader.read::<8>().map_err(truncated)?;
        let nonce = reader.array::<32>().map_err(truncated)?;
        let key_iv = reader.array::<16>().map_err(truncated)?;
        let key_rsc = reader.array::<8>().map_err(truncated)?;
        let key_identifier = reader.array::<8>().map_err(truncated)?;
        let mic = reader.array::<EAPOL_KEY_MIC_LEN>().map_err(truncated)?;
        let key_data_len = usize::from(reader.u16_be().map_err(truncated)?);
        let key_data = reader.remaining();
        if packet_type != EAPOL_PACKET_TYPE_KEY {
            return Err(EapolParseError::NotKeyPacket);
        }
        let body_len = usize::from(u16::from_be_bytes([length_high, length_low]));
        let packet_len = EAPOL_HEADER_LEN
            .checked_add(body_len)
            .ok_or(EapolParseError::LengthMismatch)?;
        if packet_len != bytes.len() || body_len < EAPOL_KEY_FIXED_LEN {
            return Err(EapolParseError::LengthMismatch);
        }
        if descriptor != RSN_KEY_DESCRIPTOR_TYPE {
            return Err(EapolParseError::NotRsnKeyDescriptor);
        }
        if key_data_len != key_data.len() {
            return Err(EapolParseError::LengthMismatch);
        }
        let mut around_mic = oer_wire::Reader::new(bytes);
        let before_mic = around_mic.take(BEFORE_MIC_LEN).map_err(truncated)?;
        let _ = around_mic.array::<EAPOL_KEY_MIC_LEN>().map_err(truncated)?;
        let after_mic = around_mic.remaining();
        Ok(Self {
            bytes,
            protocol_version,
            key_information,
            key_length,
            replay_counter,
            nonce,
            key_iv,
            key_rsc,
            key_identifier,
            mic,
            key_data,
            before_mic,
            after_mic,
        })
    }

    /// The complete packet.
    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    pub const fn protocol_version(self) -> u8 {
        self.protocol_version
    }

    pub const fn key_information(self) -> KeyInformation {
        self.key_information
    }

    pub const fn key_length(self) -> u16 {
        u16::from_be_bytes(self.key_length)
    }

    pub const fn replay_counter(self) -> u64 {
        u64::from_be_bytes(self.replay_counter)
    }

    pub const fn nonce(self) -> &'a [u8; 32] {
        self.nonce
    }

    pub const fn key_iv(self) -> &'a [u8; 16] {
        self.key_iv
    }

    /// The Key RSC field.
    pub const fn key_rsc(self) -> &'a [u8; 8] {
        self.key_rsc
    }

    pub const fn key_identifier(self) -> &'a [u8; 8] {
        self.key_identifier
    }

    pub const fn mic(self) -> &'a [u8; EAPOL_KEY_MIC_LEN] {
        self.mic
    }

    pub const fn key_data(self) -> &'a [u8] {
        self.key_data
    }

    /// The octets a Key MIC is computed over, in order: the packet with its
    /// Key MIC field zero.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 12.7.2 (Key MIC: computed over the
    /// EAPOL PDU with the Key MIC field set to 0).
    pub const fn mic_input(self) -> [&'a [u8]; 3] {
        [self.before_mic, &ZERO_MIC, self.after_mic]
    }
}

/// The fields of an EAPOL-Key packet a key holder sends; its EAPOL-Key IV
/// and Key Identifier are zero and its Key MIC is written afterwards.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EapolKeyFields<'a> {
    pub protocol_version: u8,
    pub key_information: KeyInformation,
    pub key_length: u16,
    pub replay_counter: u64,
    pub nonce: [u8; 32],
    pub key_rsc: [u8; 8],
    pub key_data: &'a [u8],
}

/// An EAPOL-Key packet these fields cannot make, or a buffer that cannot
/// hold it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapolEncodeError {
    /// The Key Data does not fit the body's 16-bit length.
    KeyDataTooLong,
    /// The buffer holds fewer octets than the packet.
    BufferTooSmall { required: usize },
}

/// One EAPOL-Key packet in an owned buffer of `N` octets: its Key MIC is
/// written in place.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EapolKeyBuffer<const N: usize> {
    bytes: [u8; N],
    length: oer_wire::PrefixLength<N>,
}

impl<const N: usize> EapolKeyBuffer<N> {
    /// Encode `fields` with a zero Key MIC; a refusal leaves nothing.
    pub fn encode(fields: &EapolKeyFields<'_>) -> Result<Self, EapolEncodeError> {
        let key_data_len =
            u16::try_from(fields.key_data.len()).map_err(|_| EapolEncodeError::KeyDataTooLong)?;
        let body_len = EAPOL_KEY_FIXED_LEN
            .checked_add(fields.key_data.len())
            .and_then(|length| u16::try_from(length).ok())
            .ok_or(EapolEncodeError::KeyDataTooLong)?;
        let required = EAPOL_HEADER_LEN
            .checked_add(usize::from(body_len))
            .ok_or(EapolEncodeError::KeyDataTooLong)?;
        let length = oer_wire::PrefixLength::new(required)
            .ok_or(EapolEncodeError::BufferTooSmall { required })?;
        let mut bytes = [0; N];
        let mut writer = oer_wire::Writer::new(&mut bytes);
        for field in [
            &[fields.protocol_version, EAPOL_PACKET_TYPE_KEY][..],
            &body_len.to_be_bytes(),
            &[RSN_KEY_DESCRIPTOR_TYPE],
            &fields.key_information.to_bytes(),
            &fields.key_length.to_be_bytes(),
            &fields.replay_counter.to_be_bytes(),
            &fields.nonce,
            &[0; 16],
            &fields.key_rsc,
            &[0; 8],
            &ZERO_MIC,
            &key_data_len.to_be_bytes(),
            fields.key_data,
        ] {
            writer
                .write(field)
                .map_err(|_| EapolEncodeError::BufferTooSmall { required })?;
        }
        Ok(Self { bytes, length })
    }

    /// The packet.
    pub fn as_bytes(&self) -> &[u8] {
        self.length.borrow(&self.bytes)
    }

    /// The packet as a checked view.
    pub fn key(&self) -> Result<EapolKey<'_>, EapolParseError> {
        EapolKey::parse(self.as_bytes())
    }

    /// Write the Key MIC field.
    pub fn set_mic(&mut self, mic: &[u8; EAPOL_KEY_MIC_LEN]) {
        // The packet holds its fixed fields, so the MIC field lies within.
        for (target, source) in self
            .bytes
            .iter_mut()
            .skip(BEFORE_MIC_LEN)
            .take(EAPOL_KEY_MIC_LEN)
            .zip(mic)
        {
            *target = *source;
        }
    }
}

/// Length of a GTK KDE for a 16-octet GTK: header, Key ID octet, reserved
/// octet, GTK.
///
/// SOURCE: IEEE Std 802.11-2020, 12.7.2, Figure 12-36 (GTK KDE format).
pub const GTK_KDE_LEN: usize = KDE_HEADER_LEN + 2 + 16;
/// Length of an IGTK KDE for a 16-octet IGTK: header, Key ID, IPN, IGTK.
///
/// SOURCE: IEEE Std 802.11-2020, 12.7.2, Figure 12-42 (IGTK KDE format).
pub const IGTK_KDE_LEN: usize = KDE_HEADER_LEN + 2 + 6 + 16;

/// A GTK KDE's data: its Key ID and Tx octet and the GTK.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GtkKde<'a> {
    pub info: GtkKdeInfo,
    pub key: &'a [u8; 16],
}

impl<'a> GtkKde<'a> {
    /// The data of a GTK KDE: `None` when its length or reserved bits are
    /// not those of a 16-octet GTK.
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        let (&[info, reserved], key) = data.split_first_chunk::<2>()?;
        let key = <&[u8; 16]>::try_from(key).ok()?;
        let info = GtkKdeInfo::from_bytes([info]);
        if reserved != 0 || info.reserved() != 0 {
            return None;
        }
        Some(Self { info, key })
    }

    /// The complete KDE.
    pub fn encode(&self) -> [u8; GTK_KDE_LEN] {
        const HEADER: [u8; KDE_HEADER_LEN] =
            match kde_header(GTK_KDE_TYPE, GTK_KDE_LEN - KDE_HEADER_LEN) {
                Some(header) => header,
                None => panic!("GTK KDE geometry"),
            };
        let mut kde = [0; GTK_KDE_LEN];
        let [info] = self.info.to_bytes();
        for (target, source) in kde
            .iter_mut()
            .zip(HEADER.iter().chain(&[info, 0]).chain(self.key))
        {
            *target = *source;
        }
        kde
    }
}

/// An IGTK KDE's data: its Key ID, the IGTK packet number and the IGTK.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IgtkKde<'a> {
    /// The little-endian Key ID field.
    pub key_id: u16,
    /// The IPN field.
    pub packet_number: [u8; 6],
    pub key: &'a [u8; 16],
}

impl<'a> IgtkKde<'a> {
    /// The data of an IGTK KDE: `None` when its length is not that of a
    /// 16-octet IGTK.
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        let (&[id0, id1], rest) = data.split_first_chunk::<2>()?;
        let (&packet_number, key) = rest.split_first_chunk::<6>()?;
        let key = <&[u8; 16]>::try_from(key).ok()?;
        Some(Self {
            key_id: u16::from_le_bytes([id0, id1]),
            packet_number,
            key,
        })
    }

    /// The complete KDE.
    pub fn encode(&self) -> [u8; IGTK_KDE_LEN] {
        const HEADER: [u8; KDE_HEADER_LEN] =
            match kde_header(IGTK_KDE_TYPE, IGTK_KDE_LEN - KDE_HEADER_LEN) {
                Some(header) => header,
                None => panic!("IGTK KDE geometry"),
            };
        let mut kde = [0; IGTK_KDE_LEN];
        let id = self.key_id.to_le_bytes();
        for (target, source) in kde.iter_mut().zip(
            HEADER
                .iter()
                .chain(&id)
                .chain(&self.packet_number)
                .chain(self.key),
        ) {
            *target = *source;
        }
        kde
    }
}

/// The padding octets a Key Data field of `unpadded` octets takes before AES
/// key wrap: to at least 16 octets and a multiple of 8, the first padding
/// octet 0xdd and the rest zero. `None` when the padded length overflows.
///
/// SOURCE: IEEE Std 802.11-2020, 12.7.2 (Key Data padding) and RFC 3394
/// (the key wrap takes at least two 64-bit blocks).
pub const fn key_data_padding(unpadded: usize) -> Option<usize> {
    let padding = match 16_usize.checked_sub(unpadded) {
        Some(short) if short > 0 => short,
        // The octets that round the length up to a multiple of eight.
        _ => unpadded.wrapping_neg() % 8,
    };
    match unpadded.checked_add(padding) {
        Some(_) => Some(padding),
        None => None,
    }
}

/// Write Key Data padding into `padding`: 0xdd, then zeros.
pub fn write_key_data_padding(padding: &mut [u8]) {
    let mut octets = padding.iter_mut();
    if let Some(first) = octets.next() {
        *first = KDE_ELEMENT_ID;
    }
    for octet in octets {
        *octet = 0;
    }
}

/// A Key Data field whose elements cannot be read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MalformedKeyData;

/// The elements of a Key Data field, up to its padding.
///
/// SOURCE: IEEE Std 802.11-2020, 12.7.2 (Key Data: when the field is
/// padded, the padding is 0xdd followed by zero or more 0x00 octets).
/// Trailing zero octets are padding as well, as an AES key wrap leaves them.
#[derive(Clone, Debug)]
pub struct KeyDataElements<'a> {
    remaining: &'a [u8],
}

impl<'a> KeyDataElements<'a> {
    pub const fn new(key_data: &'a [u8]) -> Self {
        Self {
            remaining: key_data,
        }
    }
}

impl<'a> Iterator for KeyDataElements<'a> {
    /// One complete element: identifier, length and body.
    type Item = Result<&'a [u8], MalformedKeyData>;

    fn next(&mut self) -> Option<Self::Item> {
        let (&first, after_first) = self.remaining.split_first()?;
        if self.remaining.iter().all(|byte| *byte == 0)
            || (first == KDE_ELEMENT_ID && after_first.iter().all(|byte| *byte == 0))
        {
            self.remaining = &[];
            return None;
        }
        let element = self
            .remaining
            .first_chunk::<2>()
            .and_then(|&[_, length]| usize::from(length).checked_add(2))
            .and_then(|length| self.remaining.split_at_checked(length));
        match element {
            Some((element, rest)) => {
                self.remaining = rest;
                Some(Ok(element))
            }
            None => {
                self.remaining = &[];
                Some(Err(MalformedKeyData))
            }
        }
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
    /// Message 4 of a PSK handshake: version 1, Key Information 0x030a,
    /// replay counter 9, a MIC of 0xa5 and no Key Data.
    fn message4() -> EapolKeyBuffer<128> {
        EapolKeyBuffer::encode(&EapolKeyFields {
            protocol_version: 1,
            key_information: KeyInformation::from_raw(0x030a),
            key_length: 0,
            replay_counter: 9,
            nonce: [0; 32],
            key_rsc: [0; 8],
            key_data: &[],
        })
        .unwrap()
    }

    #[test]
    fn an_encoded_packet_has_the_reference_layout_and_reads_back() {
        let mut packet = message4();
        packet.set_mic(&[0xa5; 16]);
        let bytes = packet.as_bytes();
        assert_eq!(bytes.len(), EAPOL_KEY_PACKET_LEN);
        // Header: version 1, Key, body length 95; descriptor 2; Key Info.
        assert_eq!(&bytes[..7], &[1, 3, 0, 95, 2, 0x03, 0x0a]);
        assert_eq!(&bytes[9..17], &9_u64.to_be_bytes());
        assert_eq!(&bytes[81..97], &[0xa5; 16]);
        assert_eq!(&bytes[97..], &[0, 0]);
        let key = packet.key().unwrap();
        assert_eq!(key.protocol_version(), 1);
        assert_eq!(key.key_information().raw(), 0x030a);
        assert_eq!(key.replay_counter(), 9);
        assert_eq!(key.mic(), &[0xa5; 16]);
        assert!(key.key_data().is_empty());
        // The MIC input is the packet with a zero MIC.
        let mut input = std::vec::Vec::new();
        for part in key.mic_input() {
            input.extend_from_slice(part);
        }
        let mut zeroed = bytes.to_vec();
        zeroed[81..97].fill(0);
        assert_eq!(input, zeroed);
    }

    #[test]
    fn a_packet_refuses_what_it_cannot_be() {
        let packet = message4();
        let bytes = packet.as_bytes();
        assert_eq!(
            EapolKey::parse(&bytes[..98]),
            Err(EapolParseError::Truncated)
        );
        let mut other = bytes.to_vec();
        other[1] = 0;
        assert_eq!(EapolKey::parse(&other), Err(EapolParseError::NotKeyPacket));
        let mut other = bytes.to_vec();
        other[4] = 254;
        assert_eq!(
            EapolKey::parse(&other),
            Err(EapolParseError::NotRsnKeyDescriptor)
        );
        let mut longer = bytes.to_vec();
        longer.push(0);
        assert_eq!(
            EapolKey::parse(&longer),
            Err(EapolParseError::LengthMismatch)
        );
        let mut data_len = bytes.to_vec();
        data_len[98] = 1;
        assert_eq!(
            EapolKey::parse(&data_len),
            Err(EapolParseError::LengthMismatch)
        );
        assert_eq!(
            EapolKeyBuffer::<98>::encode(&EapolKeyFields {
                protocol_version: 1,
                key_information: KeyInformation::EMPTY,
                key_length: 0,
                replay_counter: 0,
                nonce: [0; 32],
                key_rsc: [0; 8],
                key_data: &[],
            }),
            Err(EapolEncodeError::BufferTooSmall { required: 99 })
        );
    }

    #[test]
    fn group_key_kdes_have_their_reference_layout_and_refuse_other_lengths() {
        let gtk = GtkKde {
            info: GtkKdeInfo::new(GroupKeyId::new(1).unwrap(), true),
            key: &[0x11; 16],
        }
        .encode();
        assert_eq!(&gtk[..8], &[0xdd, 22, 0x00, 0x0f, 0xac, 1, 0x05, 0]);
        assert_eq!(&gtk[8..], &[0x11; 16]);
        let parsed = GtkKde::parse(&gtk[KDE_HEADER_LEN..]).unwrap();
        assert_eq!(parsed.key, &[0x11; 16]);
        assert_eq!(parsed.info.group_key_id(), GroupKeyId::new(1));
        assert_eq!(GtkKde::parse(&gtk[KDE_HEADER_LEN..23]), None);
        let mut reserved = gtk;
        reserved[7] = 1;
        assert_eq!(GtkKde::parse(&reserved[KDE_HEADER_LEN..]), None);

        let igtk = IgtkKde {
            key_id: 4,
            packet_number: [1, 2, 3, 4, 5, 6],
            key: &[0x22; 16],
        }
        .encode();
        assert_eq!(&igtk[..8], &[0xdd, 28, 0x00, 0x0f, 0xac, 9, 4, 0]);
        assert_eq!(&igtk[8..14], &[1, 2, 3, 4, 5, 6]);
        let parsed = IgtkKde::parse(&igtk[KDE_HEADER_LEN..]).unwrap();
        assert_eq!(
            (parsed.key_id, parsed.packet_number),
            (4, [1, 2, 3, 4, 5, 6])
        );
        assert_eq!(IgtkKde::parse(&igtk[KDE_HEADER_LEN..29]), None);
    }

    #[test]
    fn key_data_elements_end_at_padding_and_refuse_a_cut_element() {
        let data = [48, 2, 1, 0, 0xdd, 1, 7, 0xdd, 0, 0];
        let elements: std::vec::Vec<_> = KeyDataElements::new(&data).collect();
        assert_eq!(elements, [Ok(&data[..4]), Ok(&data[4..7])]);
        assert_eq!(KeyDataElements::new(&[0; 8]).count(), 0);
        let cut = [48, 4, 1, 0];
        let elements: std::vec::Vec<_> = KeyDataElements::new(&cut).collect();
        assert_eq!(elements, [Err(MalformedKeyData)]);
        assert_eq!(key_data_padding(0), Some(16));
        assert_eq!(key_data_padding(15), Some(1));
        assert_eq!(key_data_padding(16), Some(0));
        assert_eq!(key_data_padding(17), Some(7));
        assert_eq!(key_data_padding(usize::MAX), None);
        let mut padding = [0xa5; 3];
        write_key_data_padding(&mut padding);
        assert_eq!(padding, [0xdd, 0, 0]);
    }
}
