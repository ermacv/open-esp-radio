//! Checked management/data frame geometry shared by transmit and receive.
//!
//! Received bytes reach this module; nothing here panics on them.

use crate::sequence::SequenceControl;

/// The Frame Control field of a data or management frame of protocol
/// version 0: never a control frame, whose kinds have their own buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameControl(FrameControlFields);

/// The Protected Frame subfield of a Frame Control field: the frame body
/// is encapsulated by a cryptographic protocol, or not (IEEE Std
/// 802.11-2020, 9.2.4.1.9).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameProtection {
    Protected,
    Unprotected,
}

/// The Retry subfield of a Frame Control field: the frame is a
/// retransmission of an earlier frame, or its first transmission (IEEE Std
/// 802.11-2020, 9.2.4.1.6).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Retry {
    First,
    Retransmission,
}

impl FrameControl {
    /// The field's subfields.
    pub const fn fields(self) -> FrameControlFields {
        self.0
    }
    const DATA: u16 = 2;
    const MANAGEMENT: u16 = 0;

    /// `field`, when it is a data or management frame of version 0.
    pub const fn new(fields: FrameControlFields) -> Option<Self> {
        let kind = fields.kind().encoding() as u16;
        if fields.version() == 0 && (kind == Self::DATA || kind == Self::MANAGEMENT) {
            Some(Self(fields))
        } else {
            None
        }
    }

    /// Select the Protected Frame subfield while retaining the validated
    /// frame kind.
    pub const fn with_protected(self, protection: FrameProtection) -> Self {
        Self::of_fields(
            self.fields()
                .with_protected(matches!(protection, FrameProtection::Protected)),
        )
    }
    /// Select the Retry subfield without changing the frame's kind,
    /// protection or direction.
    pub const fn with_retry(self, retry: Retry) -> Self {
        Self::of_fields(
            self.fields()
                .with_retry(matches!(retry, Retry::Retransmission)),
        )
    }

    /// The field of `fields`, which keep a validated frame kind.
    const fn of_fields(fields: FrameControlFields) -> Self {
        Self(fields)
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

    /// A Null or a QoS Null data frame: a data frame with no frame body.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.3, Table 9-1 (data subtypes 4,
    /// Null, and 12, QoS Null).
    pub const fn is_null_data(self) -> bool {
        const NULL: u16 = 4;
        const QOS_NULL: u16 = 12;
        self.is_data() && matches!(self.subtype(), NULL | QOS_NULL)
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
/// - control: Address 1, and Address 2 for the subtypes that carry a TA
///   ([`ControlLayout`]).
///
/// Every field the frame's type holds must be complete; a captured prefix
/// that ends inside one is refused, not read as a shorter frame. A frame
/// whose address layout this view does not know (a TACK, a Control Frame
/// Extension, a reserved control subtype or an extension frame) is refused
/// as such, never read as Address 1 alone.
///
/// SOURCE: IEEE Std 802.11-2020, 9.2.3 (General frame format, Figure 9-1),
/// 9.3.2.1 (Data frame format: Address 4 with To DS and From DS set), 9.3.3.2
/// (Management frame format) and 9.3.1 (Control frames).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddressFields {
    control: FrameControlFields,
    receiver: [u8; 6],
    rest: AfterReceiver,
}

/// The fields after Address 1, as the frame's layout holds them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AfterReceiver {
    /// A control frame of Address 1 alone: CTS, Ack, Control Wrapper.
    Nothing,
    /// A control frame that carries a TA.
    Transmitter([u8; 6]),
    /// A management frame, or a data frame without Address 4: Addresses 2
    /// and 3 and Sequence Control.
    Three {
        transmitter: [u8; 6],
        address3: [u8; 6],
        sequence_control: SequenceControl,
    },
    /// A data frame with both To DS and From DS set: Address 4 as well.
    Four {
        transmitter: [u8; 6],
        address3: [u8; 6],
        sequence_control: SequenceControl,
        address4: [u8; 6],
    },
}

/// Why a frame's address fields cannot be read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressFieldsError {
    /// The frame ends inside a field its type holds.
    Truncated,
    /// A protocol version other than 0, whose layout is not defined here.
    UnknownVersion,
    /// A frame type or control subtype whose address layout this view does
    /// not know.
    UnknownLayout,
}

/// The layout Frame Control decides, before any field it governs is read.
#[derive(Clone, Copy)]
enum AddressLayout {
    Receiver,
    Transmitter,
    Three,
    Four,
}

impl AddressFields {
    /// The address fields of `frame`.
    pub fn parse(frame: &[u8]) -> Result<Self, AddressFieldsError> {
        let mut reader = oer_portable_wire::Reader::new(frame);
        let truncated = |_| AddressFieldsError::Truncated;
        let control = FrameControlFields::from_bytes(reader.read().map_err(truncated)?);
        if control.version() != 0 {
            return Err(AddressFieldsError::UnknownVersion);
        }
        // Frame Control alone decides the layout, before any field it
        // governs is read: an unknown layout is refused as such, however
        // short the frame.
        let layout = match control.kind() {
            FrameType::Management => AddressLayout::Three,
            FrameType::Data if control.to_ds() && control.from_ds() => AddressLayout::Four,
            FrameType::Data => AddressLayout::Three,
            FrameType::Control => match control_layout(control.control_subtype()) {
                ControlLayout::ReceiverAndTransmitter => AddressLayout::Transmitter,
                ControlLayout::Receiver => AddressLayout::Receiver,
                ControlLayout::Unknown => return Err(AddressFieldsError::UnknownLayout),
            },
            FrameType::Extension | FrameType::Reserved(_) => {
                return Err(AddressFieldsError::UnknownLayout);
            }
        };
        let _duration: [u8; 2] = reader.read().map_err(truncated)?;
        let receiver = reader.read().map_err(truncated)?;
        // A struct expression evaluates its fields in the order written: the
        // order of the fields in the frame.
        let rest = match layout {
            AddressLayout::Receiver => AfterReceiver::Nothing,
            AddressLayout::Transmitter => {
                AfterReceiver::Transmitter(reader.read().map_err(truncated)?)
            }
            AddressLayout::Three => AfterReceiver::Three {
                transmitter: reader.read().map_err(truncated)?,
                address3: reader.read().map_err(truncated)?,
                sequence_control: SequenceControl::from_bytes(reader.read().map_err(truncated)?),
            },
            AddressLayout::Four => AfterReceiver::Four {
                transmitter: reader.read().map_err(truncated)?,
                address3: reader.read().map_err(truncated)?,
                sequence_control: SequenceControl::from_bytes(reader.read().map_err(truncated)?),
                address4: reader.read().map_err(truncated)?,
            },
        };
        Ok(Self {
            control,
            receiver,
            rest,
        })
    }

    pub const fn control(&self) -> FrameControlFields {
        self.control
    }

    /// Address 1.
    pub const fn receiver(&self) -> [u8; 6] {
        self.receiver
    }

    /// The fields after Address 1.
    pub const fn rest(&self) -> AfterReceiver {
        self.rest
    }

    /// Address 2, in management and data frames and in a control frame
    /// carrying a TA.
    pub const fn transmitter(&self) -> Option<[u8; 6]> {
        match self.rest {
            AfterReceiver::Nothing => None,
            AfterReceiver::Transmitter(transmitter)
            | AfterReceiver::Three { transmitter, .. }
            | AfterReceiver::Four { transmitter, .. } => Some(transmitter),
        }
    }

    /// Address 3, in management and data frames.
    pub const fn address3(&self) -> Option<[u8; 6]> {
        match self.rest {
            AfterReceiver::Nothing | AfterReceiver::Transmitter(_) => None,
            AfterReceiver::Three { address3, .. } | AfterReceiver::Four { address3, .. } => {
                Some(address3)
            }
        }
    }

    /// Sequence Control, in management and data frames.
    pub const fn sequence_control(&self) -> Option<SequenceControl> {
        match self.rest {
            AfterReceiver::Nothing | AfterReceiver::Transmitter(_) => None,
            AfterReceiver::Three {
                sequence_control, ..
            }
            | AfterReceiver::Four {
                sequence_control, ..
            } => Some(sequence_control),
        }
    }

    /// Address 4, in a data frame with both To DS and From DS set.
    pub const fn address4(&self) -> Option<[u8; 6]> {
        match self.rest {
            AfterReceiver::Four { address4, .. } => Some(address4),
            _ => None,
        }
    }

    /// Every address the frame holds, in field order.
    pub fn addresses(&self) -> impl Iterator<Item = [u8; 6]> {
        [
            Some(self.receiver),
            self.transmitter(),
            self.address3(),
            self.address4(),
        ]
        .into_iter()
        .flatten()
    }
}

/// The addresses a control frame of one subtype carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlLayout {
    /// The RA alone.
    Receiver,
    /// The RA, then the TA.
    ReceiverAndTransmitter,
    /// A layout this codec does not interpret.
    Unknown,
}

/// The addresses a control frame of `subtype` carries.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.1 (Control frames): RTS, PS-Poll,
/// CF-End, BlockAckReq, BlockAck, VHT NDP Announcement and Beamforming
/// Report Poll carry RA and TA; CTS and Ack carry RA alone; Control Wrapper
/// carries Address 1, then the carried frame's fields. IEEE Std
/// 802.11ax-2021, 9.3.1 (Trigger frame format): RA and TA. TACK and the
/// Control Frame Extension subtypes are not interpreted here.
pub const fn control_layout(subtype: ControlSubtype) -> ControlLayout {
    match subtype {
        ControlSubtype::Trigger
        | ControlSubtype::BeamformingReportPoll
        | ControlSubtype::VhtNdpAnnouncement
        | ControlSubtype::BlockAckRequest
        | ControlSubtype::BlockAck
        | ControlSubtype::PsPoll
        | ControlSubtype::Rts
        | ControlSubtype::CfEnd
        | ControlSubtype::CfEndCfAck => ControlLayout::ReceiverAndTransmitter,
        ControlSubtype::Cts | ControlSubtype::Ack | ControlSubtype::ControlWrapper => {
            ControlLayout::Receiver
        }
        ControlSubtype::Tack
        | ControlSubtype::ControlFrameExtension
        | ControlSubtype::Reserved(_) => ControlLayout::Unknown,
    }
}

/// To DS and From DS of a data frame, with Address 4 exactly when both
/// are set.
///
/// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.4 (To DS and From DS subfields,
/// Table 9-4) and 9.3.2.1 (Data frame format: Address 4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataDirection {
    /// Neither bit: a frame within an IBSS or a direct link.
    Neither,
    /// To DS alone: a frame to the AP.
    ToDs,
    /// From DS alone: a frame from the AP.
    FromDs,
    /// Both bits: a mesh or WDS frame, with its Address 4.
    Wds([u8; 6]),
}

/// What a data or management header holds after Sequence Control, as its
/// type decides it. The Frame Control bits it decides are not stored
/// beside it: [`MacHeader::control`] derives them from the variant.
///
/// SOURCE: IEEE Std 802.11-2020, 9.2.3 (General frame format), 9.3.2.1
/// (Data frame format) and 9.3.3.2 (Management frame format); the Order
/// bit, 9.2.4.1.10: an HT Control field in a QoS data or management frame,
/// the StrictlyOrdered service class in a non-QoS data frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeaderLayout {
    /// A management frame: HT Control when its Order bit is set.
    Management { ht_control: Option<[u8; 4]> },
    /// A non-QoS data frame: no QoS Control and no HT Control. Its Order
    /// bit, the StrictlyOrdered class, stays among the frame's other bits.
    Data { direction: DataDirection },
    /// A QoS data frame: QoS Control, and HT Control when its Order bit is
    /// set.
    QosData {
        direction: DataDirection,
        qos: QosControl,
        ht_control: Option<[u8; 4]>,
    },
}

/// A complete MAC header with its checked variable-length boundary.
#[derive(Clone, Copy, Debug)]
pub struct DecodedHeader<'a> {
    header: MacHeader,
    frame: &'a [u8],
    body: &'a [u8],
}
impl<'a> DecodedHeader<'a> {
    /// Decode the complete MAC/QoS/HT-Control header.
    pub fn decode(frame: &'a [u8]) -> Option<Self> {
        let mut reader = oer_portable_wire::Reader::new(frame);
        let control = FrameControl::new(FrameControlFields::from_bytes(reader.read().ok()?))?;
        let fields = control.fields();
        let duration = reader.u16_le().ok()?;
        let address1 = reader.read().ok()?;
        let address2 = reader.read().ok()?;
        let address3 = reader.read().ok()?;
        let sequence_control = SequenceControl::from_bytes(reader.read().ok()?);
        let direction = |reader: &mut oer_portable_wire::Reader<'a>| -> Option<DataDirection> {
            Some(match (fields.to_ds_bit(), fields.from_ds_bit()) {
                (false, false) => DataDirection::Neither,
                (true, false) => DataDirection::ToDs,
                (false, true) => DataDirection::FromDs,
                (true, true) => DataDirection::Wds(reader.read().ok()?),
            })
        };
        let ht = |reader: &mut oer_portable_wire::Reader<'a>| -> Option<Option<[u8; 4]>> {
            if fields.order() {
                Some(Some(reader.read().ok()?))
            } else {
                Some(None)
            }
        };
        // Address 4, then QoS Control, then HT Control: the fields' order.
        let layout = if control.is_management_frame() {
            HeaderLayout::Management {
                ht_control: ht(&mut reader)?,
            }
        } else if fields.qos() {
            let direction = direction(&mut reader)?;
            let qos = QosControl::from_bytes(reader.read().ok()?);
            HeaderLayout::QosData {
                direction,
                qos,
                ht_control: ht(&mut reader)?,
            }
        } else {
            HeaderLayout::Data {
                direction: direction(&mut reader)?,
            }
        };
        Some(Self {
            header: MacHeader {
                others: MacHeader::others_of(fields, &layout),
                duration,
                address1,
                address2,
                address3,
                sequence_control,
                layout,
            },
            frame,
            body: reader.remaining(),
        })
    }
    /// The decoded header.
    pub const fn header(&self) -> MacHeader {
        self.header
    }
    /// The decoded `control` field.
    pub const fn control(&self) -> FrameControl {
        self.header.control()
    }
    /// The decoded `duration` field.
    pub const fn duration(&self) -> u16 {
        self.header.duration
    }
    /// The decoded `receiver` field.
    pub const fn receiver(&self) -> [u8; 6] {
        self.header.address1
    }
    /// The decoded `transmitter` field.
    pub const fn transmitter(&self) -> [u8; 6] {
        self.header.address2
    }
    /// The decoded `address3` field.
    pub const fn address3(&self) -> [u8; 6] {
        self.header.address3
    }
    /// The decoded `sequence_control` field.
    pub const fn sequence_control(&self) -> SequenceControl {
        self.header.sequence_control
    }
    /// The decoded `address4` field.
    pub const fn address4(&self) -> Option<[u8; 6]> {
        self.header.address4()
    }
    /// The decoded `qos` field.
    pub const fn qos(&self) -> Option<QosControl> {
        self.header.qos()
    }
    /// The decoded `ht_control` field.
    pub const fn ht_control(&self) -> Option<[u8; 4]> {
        self.header.ht_control()
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
    pub const fn field(self) -> oer_portable_wire::U8<2> {
        match self {
            Self::Normal => oer_portable_wire::U8::constant::<0>(),
            Self::NoAck => oer_portable_wire::U8::constant::<1>(),
        }
    }
}

impl QosControl {
    /// The QoS Control field of a QoS data frame, its EOSP and A-MSDU
    /// Present bits clear: a caller sets them with their fields' setters.
    pub const fn new(tid: oer_portable_wire::U8<4>, ack_policy: QosAckPolicy, high: u8) -> Self {
        Self::from_bytes([0, high])
            .with_tid(tid)
            .with_ack_policy(ack_policy.field())
    }
}

/// A data or management MAC header. Its layout ([`HeaderLayout`]) is a
/// sum by the frame's type, and the Frame Control bits the layout decides
/// are derived from it, so a header whose Frame Control disagrees with its
/// fields cannot be built.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.2.1 (Data frame format) and 9.3.3.2
/// (Management frame format): Frame Control, Duration/ID, Address 1 to
/// Address 3, Sequence Control, Address 4, QoS Control and HT Control in
/// transmission order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeader {
    /// Frame Control without the bits the layout decides: the type, the
    /// QoS subtype bit, a data frame's To DS and From DS, and the Order bit
    /// of a management or QoS data frame are clear here.
    others: FrameControlFields,
    duration: u16,
    address1: [u8; 6],
    address2: [u8; 6],
    address3: [u8; 6],
    sequence_control: SequenceControl,
    layout: HeaderLayout,
}

impl MacHeader {
    /// Decode the header at the start of `frame`.
    pub fn decode(frame: &[u8]) -> Option<Self> {
        DecodedHeader::decode(frame).map(|decoded| decoded.header)
    }

    /// `fields` without the bits `layout` decides.
    const fn others_of(fields: FrameControlFields, layout: &HeaderLayout) -> FrameControlFields {
        // A management frame's bit B7 is a bit of its subtype, and its DS
        // bits are its own: only a data frame's are the layout's.
        let fields = fields.with_type_data(false);
        match layout {
            HeaderLayout::Management { .. } => fields.with_order(false),
            HeaderLayout::Data { .. } => fields
                .with_qos(false)
                .with_to_ds_bit(false)
                .with_from_ds_bit(false),
            HeaderLayout::QosData { .. } => fields
                .with_qos(false)
                .with_to_ds_bit(false)
                .with_from_ds_bit(false)
                .with_order(false),
        }
    }

    /// The Frame Control field: the header's other bits and those its
    /// layout decides.
    pub const fn control(&self) -> FrameControl {
        const fn directed(
            fields: FrameControlFields,
            direction: DataDirection,
        ) -> FrameControlFields {
            let (to_ds, from_ds) = match direction {
                DataDirection::Neither => (false, false),
                DataDirection::ToDs => (true, false),
                DataDirection::FromDs => (false, true),
                DataDirection::Wds(_) => (true, true),
            };
            fields
                .with_type_data(true)
                .with_to_ds_bit(to_ds)
                .with_from_ds_bit(from_ds)
        }
        FrameControl::of_fields(match self.layout {
            HeaderLayout::Management { ht_control } => self.others.with_order(ht_control.is_some()),
            HeaderLayout::Data { direction } => directed(self.others, direction),
            HeaderLayout::QosData {
                direction,
                ht_control,
                ..
            } => directed(self.others, direction)
                .with_qos(true)
                .with_order(ht_control.is_some()),
        })
    }
    /// The fields after Sequence Control.
    pub const fn layout(&self) -> HeaderLayout {
        self.layout
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
    pub const fn sequence_control(&self) -> SequenceControl {
        self.sequence_control
    }
    /// Address 4 of a four-address data frame.
    pub const fn address4(&self) -> Option<[u8; 6]> {
        match self.layout {
            HeaderLayout::Data {
                direction: DataDirection::Wds(address4),
            }
            | HeaderLayout::QosData {
                direction: DataDirection::Wds(address4),
                ..
            } => Some(address4),
            _ => None,
        }
    }
    /// The QoS Control field of a QoS data frame.
    pub const fn qos(&self) -> Option<QosControl> {
        match self.layout {
            HeaderLayout::QosData { qos, .. } => Some(qos),
            _ => None,
        }
    }
    /// The HT Control field.
    pub const fn ht_control(&self) -> Option<[u8; 4]> {
        match self.layout {
            HeaderLayout::Management { ht_control } | HeaderLayout::QosData { ht_control, .. } => {
                ht_control
            }
            HeaderLayout::Data { .. } => None,
        }
    }

    /// The same header with the Protected Frame subfield `protection`.
    pub const fn with_protected(self, protection: FrameProtection) -> Self {
        Self {
            others: self
                .others
                .with_protected(matches!(protection, FrameProtection::Protected)),
            ..self
        }
    }
    /// The same header with Address 1 `address1`.
    pub const fn with_address1(self, address1: [u8; 6]) -> Self {
        Self { address1, ..self }
    }
    /// The same header with Address 3 `address3`.
    pub const fn with_address3(self, address3: [u8; 6]) -> Self {
        Self { address3, ..self }
    }
    /// The same header with the Sequence Control field `sequence_control`.
    pub const fn with_sequence_control(self, sequence_control: SequenceControl) -> Self {
        Self {
            sequence_control,
            ..self
        }
    }
    /// The same header with its QoS Control changed by `change`; a header
    /// without one, which has nothing to change, as it is.
    pub fn map_qos(self, change: impl FnOnce(QosControl) -> QosControl) -> Self {
        match self.layout {
            HeaderLayout::QosData {
                direction,
                qos,
                ht_control,
            } => Self {
                layout: HeaderLayout::QosData {
                    direction,
                    qos: change(qos),
                    ht_control,
                },
                ..self
            },
            HeaderLayout::Management { .. } | HeaderLayout::Data { .. } => self,
        }
    }

    /// The header's length in octets: 24 for Frame Control, Duration/ID,
    /// three addresses and Sequence Control, then 6 for Address 4, 2 for
    /// QoS Control and 4 for HT Control.
    pub const fn header_len(&self) -> usize {
        match self.layout {
            HeaderLayout::Management { ht_control: None } => 24,
            HeaderLayout::Management {
                ht_control: Some(_),
            } => 28,
            HeaderLayout::Data {
                direction: DataDirection::Wds(_),
            } => 30,
            HeaderLayout::Data { .. } => 24,
            HeaderLayout::QosData {
                direction,
                ht_control,
                ..
            } => match (
                matches!(direction, DataDirection::Wds(_)),
                ht_control.is_some(),
            ) {
                (false, false) => 26,
                (false, true) => 30,
                (true, false) => 32,
                (true, true) => 36,
            },
        }
    }

    /// The header's octets in transmission order (IEEE Std 802.11-2020,
    /// 9.2.3: Frame Control, Duration/ID, Address 1 to 3, Sequence Control,
    /// Address 4, QoS Control, HT Control), [`Self::header_len`] of them.
    pub fn octets(&self) -> impl Iterator<Item = u8> + use<> {
        self.control()
            .fields()
            .to_bytes()
            .into_iter()
            .chain(self.duration.to_le_bytes())
            .chain(self.address1)
            .chain(self.address2)
            .chain(self.address3)
            .chain(self.sequence_control.to_bytes())
            .chain(self.address4().into_iter().flatten())
            .chain(self.qos().map(QosControl::to_bytes).into_iter().flatten())
            .chain(self.ht_control().into_iter().flatten())
    }

    /// Write the header at the start of `out` and return its length. A
    /// shorter `out` is refused before any octet is written.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, oer_portable_wire::Error> {
        let len = self.header_len();
        let Some(header) = out.get_mut(..len) else {
            return Err(oer_portable_wire::Error::Truncated {
                required: len,
                remaining: out.len(),
            });
        };
        for (place, octet) in header.iter_mut().zip(self.octets()) {
            *place = octet;
        }
        Ok(len)
    }
}

oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.1 (Frame Control field).
    pub struct FrameControlFields[2] (little) {
        pub version, with_version: u8 = 0..2;
        pub kind, with_kind: [FrameType] = 2..4;
        pub subtype, with_subtype: u16 = 4..8;
        pub management_subtype, with_management_subtype: [ManagementSubtype] = 4..8, view;
        pub control_subtype, with_control_subtype: [ControlSubtype] = 4..8, view;
        pub type_and_subtype, with_type_and_subtype: u8 = 2..8, view;
        /// Type bit B3: set in a data (10) or extension (11) frame, clear
        /// in a management (00) or control (01) one.
        /// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.3 (Type and Subtype
        /// subfields).
        pub type_data, with_type_data: bool = 3..4, view;
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
        /// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.4 (To DS and From DS
        /// subfields).
        pub to_ds_bit, with_to_ds_bit: bool = 8..9, view;
        pub from_ds_bit, with_from_ds_bit: bool = 9..10, view;
        pub more_fragments, with_more_fragments: bool = 10..11;
        pub retry, with_retry: bool = 11..12;
        pub power_management, with_power_management: bool = 12..13;
        pub more_data, with_more_data: bool = 13..14;
        pub protected, with_protected: bool = 14..15;
        pub order, with_order: bool = 15..16;
    }
}
oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.5 (QoS Control field).
    pub struct QosControl[2] (little) {
        pub tid, with_tid: (u8) = 0..4;
        pub eosp, with_eosp: bool = 4..5;
        pub ack_policy, with_ack_policy: (u8) = 5..7;
        pub amsdu, with_amsdu: bool = 7..8;
        pub upper, with_upper: u8 = 8..16;
    }
}
oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.2.4.6 (HT Control field).
    pub struct HtControl[4] (little) {
        pub vht, with_vht: bool = 0..1;
        pub he, with_he: bool = 1..2;
        pub control, with_control: u32 = 2..32;
    }
}

oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.3 (Type and Subtype subfields).
    pub enum FrameType: u8 { Management = 0, Control = 1, Data = 2, Extension = 3 }
}

oer_portable_wire::bitfield! {
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

oer_portable_wire::bitfield! {
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

oer_portable_wire::bitfield! {
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
pub fn sequence_control(mpdu: &[u8]) -> Option<SequenceControl> {
    mpdu.get(SEQUENCE_CONTROL_OFFSET..)?
        .first_chunk::<2>()
        .copied()
        .map(SequenceControl::from_bytes)
}

/// Write the Sequence Control field of a data or management MPDU; `None`,
/// without a change, for an MPDU shorter than the field's end.
pub fn write_sequence_control(mpdu: &mut [u8], sequence_control: SequenceControl) -> Option<()> {
    *mpdu
        .get_mut(SEQUENCE_CONTROL_OFFSET..)?
        .first_chunk_mut::<2>()? = sequence_control.to_bytes();
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
        let field = SequenceControl::from_bytes([0x34, 0x12]);
        let mut mpdu = [0_u8; 24];
        write_sequence_control(&mut mpdu, field).unwrap();
        assert_eq!(&mpdu[22..], &[0x34, 0x12]);
        assert_eq!(sequence_control(&mpdu), Some(field));
        let mut short = [0_u8; 23];
        assert_eq!(write_sequence_control(&mut short, field), None);
        assert_eq!(short, [0; 23]);
        assert_eq!(sequence_control(&short), None);
    }
}
