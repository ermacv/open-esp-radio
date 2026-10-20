//! Checked management/data frame geometry shared by transmit and receive.
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

/// Plain station-to-DS Data frame control.
pub const DATA_TO_DS: u16 = 0x0108;
/// QoS station-to-DS Data frame control.
pub const QOS_DATA_TO_DS: u16 = 0x0188;

/// The Frame Control field of a data or management frame of protocol
/// version 0: never a control frame, whose kinds have their own buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameControl(u16);

impl FrameControl {
    const fn fields(self) -> FrameControlFields {
        FrameControlFields::from_bytes(self.0.to_le_bytes())
    }
    const DATA: u16 = 2;
    const MANAGEMENT: u16 = 0;

    /// `field`, when it is a data or management frame of version 0.
    pub const fn new(field: u16) -> Option<Self> {
        let fields = FrameControlFields::from_bytes(field.to_le_bytes());
        let kind = fields.kind().encoding() as u16;
        if fields.version() == 0 && (kind == Self::DATA || kind == Self::MANAGEMENT) {
            Some(Self(field))
        } else {
            None
        }
    }

    /// The field.
    pub const fn get(self) -> u16 {
        self.0
    }

    /// Select the Protected flag while retaining the validated frame kind.
    pub const fn with_protected(self, protected: bool) -> Self {
        Self(u16::from_le_bytes(
            FrameControlFields::from_bytes(self.0.to_le_bytes())
                .with_protected(protected)
                .to_bytes(),
        ))
    }
    /// Select Retry without changing the frame's kind, protection or direction.
    pub const fn with_retry(self, retry: bool) -> Self {
        Self(u16::from_le_bytes(
            FrameControlFields::from_bytes(self.0.to_le_bytes())
                .with_retry(retry)
                .to_bytes(),
        ))
    }
    /// This MPDU repeats an earlier transmission.
    pub const fn retry(self) -> bool {
        self.fields().retry()
    }
    /// A data frame.
    pub const fn is_data(self) -> bool {
        self.fields().kind().encoding() as u16 == Self::DATA
    }
    /// Payload has CCMP protection.
    pub const fn protected(self) -> bool {
        self.fields().protected()
    }
    /// The DS direction bits, ToDS followed by FromDS.
    pub const fn ds(self) -> u16 {
        self.fields().ds()
    }
    /// The subtype within its frame kind.
    pub const fn subtype(self) -> u16 {
        self.fields().subtype()
    }
    /// Another fragment follows.
    pub const fn more_fragments(self) -> bool {
        self.fields().more_fragments()
    }

    pub const fn is_action_no_ack(self) -> bool {
        self.fields().type_and_subtype() == 0b11_1000
    }

    pub const fn is_qos_data(self) -> bool {
        self.fields().kind().encoding() as u16 == Self::DATA && self.fields().qos()
    }

    pub const fn is_four_address(self) -> bool {
        self.fields().kind().encoding() as u16 == Self::DATA && self.fields().ds() == 3
    }

    /// Whether the frame carries an HT Control field: the Order bit of a
    /// QoS data or management frame; in a non-QoS data frame the bit
    /// requests strictly ordered service instead.
    pub const fn has_ht_control(self) -> bool {
        self.fields().order()
            && (self.is_qos_data() || self.fields().kind().encoding() as u16 == Self::MANAGEMENT)
    }
}

/// A complete MAC header with its checked variable-length boundary.
#[derive(Clone, Copy, Debug)]
pub struct DecodedHeader<'a> {
    control: FrameControl,
    duration: u16,
    receiver: [u8; 6],
    transmitter: [u8; 6],
    address3: [u8; 6],
    sequence_control: u16,
    address4: Option<[u8; 6]>,
    qos: Option<u16>,
    ht_control: Option<[u8; 4]>,
    frame: &'a [u8],
    body: &'a [u8],
}
impl<'a> DecodedHeader<'a> {
    /// Decode the complete MAC/QoS/HT-Control header.
    pub fn decode(frame: &'a [u8]) -> Option<Self> {
        let original = frame;
        let mut reader = oer_wire::Reader::new(frame);
        let control = FrameControl::new(u16::from_le_bytes(reader.read().ok()?))?;
        let duration = u16::from_le_bytes(reader.read().ok()?);
        let receiver = reader.read().ok()?;
        let transmitter = reader.read().ok()?;
        let address3 = reader.read().ok()?;
        let sequence_control = u16::from_le_bytes(reader.read().ok()?);
        let address4 = if control.is_four_address() {
            Some(reader.read().ok()?)
        } else {
            None
        };
        let qos = if control.is_qos_data() {
            Some(u16::from_le_bytes(reader.read().ok()?))
        } else {
            None
        };
        let ht_control = if control.has_ht_control() {
            Some(reader.read().ok()?)
        } else {
            None
        };
        Some(Self {
            control,
            duration,
            receiver,
            transmitter,
            address3,
            sequence_control,
            address4,
            qos,
            ht_control,
            frame: original,
            body: reader.remaining(),
        })
    }
    /// The decoded `control` field.
    pub const fn control(&self) -> FrameControl {
        self.control
    }
    /// The decoded `duration` field.
    pub const fn duration(&self) -> u16 {
        self.duration
    }
    /// The decoded `receiver` field.
    pub const fn receiver(&self) -> [u8; 6] {
        self.receiver
    }
    /// The decoded `transmitter` field.
    pub const fn transmitter(&self) -> [u8; 6] {
        self.transmitter
    }
    /// The decoded `address3` field.
    pub const fn address3(&self) -> [u8; 6] {
        self.address3
    }
    /// The decoded `sequence_control` field.
    pub const fn sequence_control(&self) -> u16 {
        self.sequence_control
    }
    /// The decoded `address4` field.
    pub const fn address4(&self) -> Option<[u8; 6]> {
        self.address4
    }
    /// The decoded `qos` field.
    pub const fn qos(&self) -> Option<u16> {
        self.qos
    }
    /// The decoded `ht_control` field.
    pub const fn ht_control(&self) -> Option<[u8; 4]> {
        self.ht_control
    }
    /// The complete MPDU whose header was decoded.
    pub const fn frame(&self) -> &'a [u8] {
        self.frame
    }
    /// Bytes following all MAC header fields.
    pub const fn body(&self) -> &'a [u8] {
        self.body
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.1 (Frame Control field).
    pub struct FrameControlFields[2] (little) {
        pub version, with_version: u8 = 0..2;
        pub kind, with_kind: [FrameType] = 2..4;
        pub subtype, with_subtype: u16 = 4..8;
        pub type_and_subtype, with_type_and_subtype: u8 = 2..8, view;
        pub qos, with_qos: bool = 7..8, view;
        pub ds, with_ds: u16 = 8..10;
        pub more_fragments, with_more_fragments: bool = 10..11;
        pub retry, with_retry: bool = 11..12;
        pub power_management, with_power_management: bool = 12..13;
        pub more_data, with_more_data: bool = 13..14;
        pub protected, with_protected: bool = 14..15;
        pub order, with_order: bool = 15..16;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.5 (QoS Control field).
    pub struct QosControl[2] (little) {
        pub tid, with_tid: u8 = 0..4;
        pub eosp, with_eosp: bool = 4..5;
        pub ack_policy, with_ack_policy: u8 = 5..7;
        pub amsdu, with_amsdu: bool = 7..8;
        pub upper, with_upper: u8 = 8..16;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.2.4.6 (HT Control field).
    pub struct HtControl[4] (little) {
        pub vht, with_vht: bool = 0..1;
        pub he, with_he: bool = 1..2;
        pub control, with_control: u32 = 2..32;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.3 (Type and Subtype subfields).
    pub enum FrameType: u8 { Management = 0, Control = 1, Data = 2, Extension = 3 }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.4 (To DS and From DS fields).
    pub struct DsDirection[1] (little) {
        pub to_ds, with_to_ds: bool = 0..1;
        pub from_ds, with_from_ds: bool = 1..2;
    }
}

#[cfg(test)]
mod fuzz_tests;

impl FrameControlFields {
    pub const fn from_raw(raw: u16) -> Self {
        Self::from_bytes(raw.to_le_bytes())
    }
    pub const fn raw(self) -> u16 {
        u16::from_le_bytes(self.to_bytes())
    }
    pub const fn to_ds(self) -> bool {
        DsDirection::from_bytes([self.ds() as u8]).to_ds()
    }
    pub const fn from_ds(self) -> bool {
        DsDirection::from_bytes([self.ds() as u8]).from_ds()
    }
}
