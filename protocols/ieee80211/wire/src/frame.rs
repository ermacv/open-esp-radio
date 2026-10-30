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
    /// A management frame.
    pub const fn is_management_frame(self) -> bool {
        self.fields().kind().encoding() as u16 == Self::MANAGEMENT
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

/// The address fields of a protocol-version-0 MPDU, present as its type,
/// subtype and DS bits define them:
///
/// - management: Addresses 1 to 3 and Sequence Control;
/// - data: the same, and Address 4 when both To DS and From DS are set;
/// - control: Address 1, and Address 2 for the subtypes that carry a TA;
/// - extension: Address 1 alone, as this view interprets no extension layout.
///
/// Every field the frame's type holds must be complete; a captured prefix
/// that ends inside one is refused, not read as a shorter frame.
///
/// SOURCE: IEEE Std 802.11-2020, 9.2.3 (General frame format, Figure 9-1),
/// 9.3.2.1 (Data frame format: Address 4 with To DS and From DS set), 9.3.3.2
/// (Management frame format) and 9.3.1 (Control frames: BlockAckReq,
/// BlockAck, PS-Poll, RTS, CF-End and CF-End +CF-Ack carry a TA).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddressFields {
    control: FrameControlFields,
    receiver: [u8; 6],
    transmitter: Option<[u8; 6]>,
    address3: Option<[u8; 6]>,
    sequence_control: Option<u16>,
    address4: Option<[u8; 6]>,
}

/// Why a frame's address fields cannot be read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressFieldsError {
    /// The frame ends inside a field its type holds.
    Truncated,
    /// A protocol version other than 0, whose layout is not defined here.
    UnknownVersion,
}

impl AddressFields {
    /// The address fields of `frame`.
    pub fn parse(frame: &[u8]) -> Result<Self, AddressFieldsError> {
        let mut reader = oer_wire::Reader::new(frame);
        let truncated = |_| AddressFieldsError::Truncated;
        let control = FrameControlFields::from_bytes(reader.read().map_err(truncated)?);
        if control.version() != 0 {
            return Err(AddressFieldsError::UnknownVersion);
        }
        let _duration: [u8; 2] = reader.read().map_err(truncated)?;
        let receiver = reader.read().map_err(truncated)?;
        let mut fields = Self {
            control,
            receiver,
            transmitter: None,
            address3: None,
            sequence_control: None,
            address4: None,
        };
        match control.kind() {
            FrameType::Management | FrameType::Data => {
                fields.transmitter = Some(reader.read().map_err(truncated)?);
                fields.address3 = Some(reader.read().map_err(truncated)?);
                fields.sequence_control =
                    Some(u16::from_le_bytes(reader.read().map_err(truncated)?));
                if matches!(control.kind(), FrameType::Data) && control.to_ds() && control.from_ds()
                {
                    fields.address4 = Some(reader.read().map_err(truncated)?);
                }
            }
            FrameType::Control if control_carries_transmitter(control.control_subtype()) => {
                fields.transmitter = Some(reader.read().map_err(truncated)?);
            }
            FrameType::Control | FrameType::Extension | FrameType::Reserved(_) => {}
        }
        Ok(fields)
    }

    pub const fn control(&self) -> FrameControlFields {
        self.control
    }

    /// Address 1.
    pub const fn receiver(&self) -> [u8; 6] {
        self.receiver
    }

    /// Address 2, in management and data frames and in a control frame
    /// carrying a TA.
    pub const fn transmitter(&self) -> Option<[u8; 6]> {
        self.transmitter
    }

    /// Address 3, in management and data frames.
    pub const fn address3(&self) -> Option<[u8; 6]> {
        self.address3
    }

    /// Sequence Control, in management and data frames.
    pub const fn sequence_control(&self) -> Option<u16> {
        self.sequence_control
    }

    /// Address 4, in a data frame with both To DS and From DS set.
    pub const fn address4(&self) -> Option<[u8; 6]> {
        self.address4
    }

    /// Every address the frame holds, in field order.
    pub fn addresses(&self) -> impl Iterator<Item = [u8; 6]> {
        [
            Some(self.receiver),
            self.transmitter,
            self.address3,
            self.address4,
        ]
        .into_iter()
        .flatten()
    }
}

/// Whether a control frame of `subtype` carries a TA after its RA.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.1 (Control frames) and Table 9-1.
const fn control_carries_transmitter(subtype: ControlSubtype) -> bool {
    matches!(
        subtype,
        ControlSubtype::BlockAckRequest
            | ControlSubtype::BlockAck
            | ControlSubtype::PsPoll
            | ControlSubtype::Rts
            | ControlSubtype::CfEnd
            | ControlSubtype::CfEndCfAck
    )
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

/// The Ack Policy a lower-MAC frame writes into its QoS Control field.
/// SOURCE: IEEE Std 802.11-2020, 9.2.4.5.4 (Ack Policy subfield), Table
/// 9-11: Normal Ack or Implicit BAR is 0, No Ack is 1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QosAckPolicy {
    /// The addressed receiver acknowledges this frame.
    Normal,
    /// No acknowledgement is solicited.
    NoAck,
}

impl QosAckPolicy {
    /// The two-bit subfield.
    pub const fn field(self) -> oer_wire::U8<2> {
        match self {
            Self::Normal => oer_wire::U8::constant::<0>(),
            Self::NoAck => oer_wire::U8::constant::<1>(),
        }
    }
}

impl QosControl {
    /// The QoS Control field of a QoS data frame.
    pub const fn new(
        tid: oer_wire::U8<4>,
        end_of_service_period: bool,
        ack_policy: QosAckPolicy,
        amsdu_present: bool,
        high: u8,
    ) -> Self {
        Self::from_bytes([0, high])
            .with_tid(tid)
            .with_eosp(end_of_service_period)
            .with_ack_policy(ack_policy.field())
            .with_amsdu(amsdu_present)
    }
}

/// A data or management MAC header whose fields agree with its Frame
/// Control: a fourth address exactly when To DS and From DS are both set, a
/// QoS Control exactly in a QoS data frame, an HT Control exactly when the
/// Order bit marks one ([`FrameControl::has_ht_control`]).
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.2.1 (Data frame format) and 9.3.3.2
/// (Management frame format): Frame Control, Duration/ID, Address 1 to
/// Address 3, Sequence Control, Address 4, QoS Control and HT Control in
/// transmission order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeader {
    control: FrameControl,
    duration: u16,
    address1: [u8; 6],
    address2: [u8; 6],
    address3: [u8; 6],
    sequence_control: u16,
    address4: Option<[u8; 6]>,
    qos: Option<QosControl>,
    ht_control: Option<[u8; 4]>,
}

impl MacHeader {
    /// The header, when its optional fields agree with `control`.
    #[allow(clippy::too_many_arguments, reason = "the fields of a MAC header")]
    pub const fn new(
        control: FrameControl,
        duration: u16,
        address1: [u8; 6],
        address2: [u8; 6],
        address3: [u8; 6],
        sequence_control: u16,
        address4: Option<[u8; 6]>,
        qos: Option<QosControl>,
        ht_control: Option<[u8; 4]>,
    ) -> Option<Self> {
        if address4.is_some() == control.is_four_address()
            && qos.is_some() == control.is_qos_data()
            && ht_control.is_some() == control.has_ht_control()
        {
            Some(Self {
                control,
                duration,
                address1,
                address2,
                address3,
                sequence_control,
                address4,
                qos,
                ht_control,
            })
        } else {
            None
        }
    }

    /// Decode the header at the start of `frame`.
    pub fn decode(frame: &[u8]) -> Option<Self> {
        let header = DecodedHeader::decode(frame)?;
        Self::new(
            header.control(),
            header.duration(),
            header.receiver(),
            header.transmitter(),
            header.address3(),
            header.sequence_control(),
            header.address4(),
            header
                .qos()
                .map(|field| QosControl::from_bytes(field.to_le_bytes())),
            header.ht_control(),
        )
    }

    /// The Frame Control field.
    pub const fn control(&self) -> FrameControl {
        self.control
    }
    /// The Duration/ID field.
    pub const fn duration(&self) -> u16 {
        self.duration
    }
    /// Address 1, the receiver.
    pub const fn address1(&self) -> [u8; 6] {
        self.address1
    }
    /// Address 2, the transmitter.
    pub const fn address2(&self) -> [u8; 6] {
        self.address2
    }
    /// Address 3.
    pub const fn address3(&self) -> [u8; 6] {
        self.address3
    }
    /// The Sequence Control field.
    pub const fn sequence_control(&self) -> u16 {
        self.sequence_control
    }
    /// Address 4 of a four-address data frame.
    pub const fn address4(&self) -> Option<[u8; 6]> {
        self.address4
    }
    /// The QoS Control field of a QoS data frame.
    pub const fn qos(&self) -> Option<QosControl> {
        self.qos
    }
    /// The HT Control field.
    pub const fn ht_control(&self) -> Option<[u8; 4]> {
        self.ht_control
    }

    /// The same header with the Protected flag `protected`.
    pub const fn with_protected(mut self, protected: bool) -> Self {
        self.control = self.control.with_protected(protected);
        self
    }
    /// The same header with Address 1 `address1`.
    pub const fn with_address1(mut self, address1: [u8; 6]) -> Self {
        self.address1 = address1;
        self
    }
    /// The same QoS data header with `qos`; `None` for a header without a
    /// QoS Control field.
    pub const fn with_qos(mut self, qos: QosControl) -> Option<Self> {
        if self.qos.is_some() {
            self.qos = Some(qos);
            Some(self)
        } else {
            None
        }
    }

    /// The header's length in octets.
    pub const fn header_len(&self) -> usize {
        match (
            self.address4.is_some(),
            self.qos.is_some(),
            self.ht_control.is_some(),
        ) {
            (false, false, false) => 24,
            (false, true, false) => 26,
            (false, false, true) => 28,
            (false, true, true) | (true, false, false) => 30,
            (true, true, false) => 32,
            (true, false, true) => 34,
            (true, true, true) => 36,
        }
    }

    /// Write the header at the start of `out` and return its length. A
    /// shorter `out` is refused before any octet is written.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, oer_wire::Error> {
        let len = self.header_len();
        if out.len() < len {
            return Err(oer_wire::Error::Truncated {
                required: len,
                remaining: out.len(),
            });
        }
        let mut writer = oer_wire::Writer::new(out);
        writer.u16_le(self.control.get())?;
        writer.u16_le(self.duration)?;
        writer.write(&self.address1)?;
        writer.write(&self.address2)?;
        writer.write(&self.address3)?;
        writer.u16_le(self.sequence_control)?;
        if let Some(address4) = self.address4 {
            writer.write(&address4)?;
        }
        if let Some(qos) = self.qos {
            writer.write(&qos.to_bytes())?;
        }
        if let Some(ht_control) = self.ht_control {
            writer.write(&ht_control)?;
        }
        Ok(len)
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.1 (Frame Control field).
    pub struct FrameControlFields[2] (little) {
        pub version, with_version: u8 = 0..2;
        pub kind, with_kind: [FrameType] = 2..4;
        pub subtype, with_subtype: u16 = 4..8;
        pub management_subtype, with_management_subtype: [ManagementSubtype] = 4..8, view;
        pub control_subtype, with_control_subtype: [ControlSubtype] = 4..8, view;
        pub type_and_subtype, with_type_and_subtype: u8 = 2..8, view;
        /// Data subtype bit B6: the frame carries no Frame Body (Null and
        /// CF-only subtypes).
        /// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.3 (Type and Subtype
        /// subfields).
        pub no_data, with_no_data: bool = 6..7, view;
        /// Data subtype bits B4 and B5: CF-Ack and CF-Poll.
        /// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.3 (Type and Subtype
        /// subfields).
        pub cf_ack, with_cf_ack: bool = 4..5, view;
        pub cf_poll, with_cf_poll: bool = 5..6, view;
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
        pub tid, with_tid: (u8) = 0..4;
        pub eosp, with_eosp: bool = 4..5;
        pub ack_policy, with_ack_policy: (u8) = 5..7;
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
    /// The subtype of a management frame.
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.3, Table 9-1 (Valid type and
    /// subtype combinations, type 00).
    pub enum ManagementSubtype: u8 {
        AssociationRequest = 0,
        AssociationResponse = 1,
        ReassociationRequest = 2,
        ReassociationResponse = 3,
        ProbeRequest = 4,
        ProbeResponse = 5,
        TimingAdvertisement = 6,
        Beacon = 8,
        Atim = 9,
        Disassociation = 10,
        Authentication = 11,
        Deauthentication = 12,
        Action = 13,
        ActionNoAck = 14
    }
}

oer_wire::bitfield! {
    /// The subtype of a control frame.
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.3, Table 9-1 (Valid type and
    /// subtype combinations, type 01); IEEE Std 802.11ax-2021 Table 9-1
    /// (Trigger, TACK).
    pub enum ControlSubtype: u8 {
        Trigger = 2,
        Tack = 3,
        BeamformingReportPoll = 4,
        VhtNdpAnnouncement = 5,
        ControlFrameExtension = 6,
        ControlWrapper = 7,
        BlockAckRequest = 8,
        BlockAck = 9,
        PsPoll = 10,
        Rts = 11,
        Cts = 12,
        Ack = 13,
        CfEnd = 14,
        CfEndCfAck = 15
    }
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
#[cfg(test)]
mod header_tests;

impl FrameControlFields {
    pub const fn from_raw(raw: u16) -> Self {
        Self::from_bytes(raw.to_le_bytes())
    }
    /// The Frame Control field of a protocol-version-0 management frame of
    /// `subtype`, every flag clear; `None` for a reserved encoding wider
    /// than the four-bit subtype.
    pub const fn management(subtype: ManagementSubtype) -> Option<Self> {
        Self::of_kind(FrameType::Management, subtype.encoding())
    }
    /// The Frame Control field of a protocol-version-0 control frame of
    /// `subtype`, every flag clear; `None` for a reserved encoding wider
    /// than the four-bit subtype.
    pub const fn control(subtype: ControlSubtype) -> Option<Self> {
        Self::of_kind(FrameType::Control, subtype.encoding())
    }
    const fn of_kind(kind: FrameType, subtype: u8) -> Option<Self> {
        let Ok(fields) = Self::from_bytes([0; 2]).with_kind(kind) else {
            return None;
        };
        match fields.with_subtype(subtype as u16) {
            Ok(fields) => Some(fields),
            Err(_) => None,
        }
    }
    /// Whether this is a management frame of `subtype`.
    pub const fn is_management(self, subtype: ManagementSubtype) -> bool {
        matches!(self.kind(), FrameType::Management)
            && self.management_subtype().encoding() == subtype.encoding()
    }
    /// Whether this is a control frame of `subtype`.
    pub const fn is_control(self, subtype: ControlSubtype) -> bool {
        matches!(self.kind(), FrameType::Control)
            && self.control_subtype().encoding() == subtype.encoding()
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

/// The longest time a Duration field reserves, in microseconds.
///
/// SOURCE: IEEE Std 802.11-2020, 9.2.4.2 (Duration/ID field: a duration
/// value of 0 to 32 767 microseconds).
pub const MAX_DURATION_MICROS: u16 = 32_767;

/// Offset of the Sequence Control field in a data or management MAC header.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.2.1 (Data frame format) and 9.3.3.2
/// (Management frame format): Sequence Control follows Frame Control,
/// Duration and Addresses 1 to 3.
pub const SEQUENCE_CONTROL_OFFSET: usize = 22;

/// The Sequence Control field of a data or management MPDU; `None` for an
/// MPDU shorter than the field's end.
pub fn sequence_control(mpdu: &[u8]) -> Option<u16> {
    mpdu.get(SEQUENCE_CONTROL_OFFSET..)?
        .first_chunk::<2>()
        .copied()
        .map(u16::from_le_bytes)
}

/// Write the Sequence Control field of a data or management MPDU; `None`,
/// without a change, for an MPDU shorter than the field's end.
pub fn write_sequence_control(mpdu: &mut [u8], sequence_control: u16) -> Option<()> {
    *mpdu
        .get_mut(SEQUENCE_CONTROL_OFFSET..)?
        .first_chunk_mut::<2>()? = sequence_control.to_le_bytes();
    Some(())
}

/// Mark an encoded MPDU as a retransmission, preserving its other bits.
/// A short frame is refused without mutation.
pub fn set_retry_bit(mpdu: &mut [u8]) -> bool {
    let Some(field) = mpdu.first_chunk_mut::<2>() else {
        return false;
    };
    *field = FrameControlFields::from_bytes(*field)
        .with_retry(true)
        .to_bytes();
    true
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    #[test]
    fn marking_a_retry_preserves_other_frame_fields_and_refuses_a_short_header() {
        let mut mpdu = [0x88, 0x01, 0x7b, 0xc2];
        assert!(set_retry_bit(&mut mpdu));
        assert_eq!(mpdu, [0x88, 0x09, 0x7b, 0xc2]);
        assert!(set_retry_bit(&mut mpdu));
        assert_eq!(mpdu, [0x88, 0x09, 0x7b, 0xc2]);
        let mut short = [0x88];
        assert!(!set_retry_bit(&mut short));
        assert_eq!(short, [0x88]);
    }

    #[test]
    fn sequence_control_round_trips_and_a_short_mpdu_is_refused_unchanged() {
        let mut mpdu = [0_u8; 24];
        write_sequence_control(&mut mpdu, 0x1234).unwrap();
        assert_eq!(&mpdu[22..], &[0x34, 0x12]);
        assert_eq!(sequence_control(&mpdu), Some(0x1234));
        let mut short = [0_u8; 23];
        assert_eq!(write_sequence_control(&mut short, 0x1234), None);
        assert_eq!(short, [0; 23]);
        assert_eq!(sequence_control(&short), None);
    }
}
