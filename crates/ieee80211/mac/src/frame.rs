//! Checked management/data frame geometry shared by transmit and receive.

/// Plain station-to-DS Data frame control.
pub const DATA_TO_DS: u16 = 0x0108;
/// QoS station-to-DS Data frame control.
pub const QOS_DATA_TO_DS: u16 = 0x0188;

/// The Frame Control field of a data or management frame of protocol
/// version 0: never a control frame, whose kinds have their own buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameControl(u16);

impl FrameControl {
    const DATA: u16 = 2;
    const MANAGEMENT: u16 = 0;
    const TYPE_AND_SUBTYPE: u16 = (0b11 << 2) | (0b1111 << 4);
    const ACTION_NO_ACK: u16 = 0b1110 << 4;

    /// `field`, when it is a data or management frame of version 0.
    pub const fn new(field: u16) -> Option<Self> {
        let kind = (field >> 2) & 0b11;
        if field & 0b11 == 0 && (kind == Self::DATA || kind == Self::MANAGEMENT) {
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
        Self((self.0 & !(1 << 14)) | if protected { 1 << 14 } else { 0 })
    }
    /// Select Retry without changing the frame's kind, protection or direction.
    pub const fn with_retry(self, retry: bool) -> Self {
        Self((self.0 & !(1 << 11)) | if retry { 1 << 11 } else { 0 })
    }
    /// This MPDU repeats an earlier transmission.
    pub const fn retry(self) -> bool {
        self.0 & (1 << 11) != 0
    }
    /// A data frame.
    pub const fn is_data(self) -> bool {
        (self.0 >> 2) & 3 == Self::DATA
    }
    /// Payload has CCMP protection.
    pub const fn protected(self) -> bool {
        self.0 & (1 << 14) != 0
    }
    /// The DS direction bits, ToDS followed by FromDS.
    pub const fn ds(self) -> u16 {
        (self.0 >> 8) & 3
    }
    /// The subtype within its frame kind.
    pub const fn subtype(self) -> u16 {
        (self.0 >> 4) & 15
    }
    /// Another fragment follows.
    pub const fn more_fragments(self) -> bool {
        self.0 & (1 << 10) != 0
    }

    pub const fn is_action_no_ack(self) -> bool {
        self.0 & Self::TYPE_AND_SUBTYPE == Self::ACTION_NO_ACK
    }

    pub const fn is_qos_data(self) -> bool {
        (self.0 >> 2) & 0b11 == Self::DATA && self.0 & (1 << 7) != 0
    }

    pub const fn is_four_address(self) -> bool {
        (self.0 >> 2) & 0b11 == Self::DATA && self.0 & (0b11 << 8) == 0b11 << 8
    }

    /// Whether the frame carries an HT Control field: the Order bit of a
    /// QoS data or management frame; in a non-QoS data frame the bit
    /// requests strictly ordered service instead.
    pub const fn has_ht_control(self) -> bool {
        self.0 & (1 << 15) != 0 && (self.is_qos_data() || (self.0 >> 2) & 0b11 == Self::MANAGEMENT)
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
    pub fn decode(mut frame: &'a [u8]) -> Option<Self> {
        fn take<const N: usize>(frame: &mut &[u8]) -> Option<[u8; N]> {
            let (field, rest) = frame.split_first_chunk()?;
            *frame = rest;
            Some(*field)
        }
        let original = frame;
        let control = FrameControl::new(u16::from_le_bytes(take(&mut frame)?))?;
        let duration = u16::from_le_bytes(take(&mut frame)?);
        let receiver = take(&mut frame)?;
        let transmitter = take(&mut frame)?;
        let address3 = take(&mut frame)?;
        let sequence_control = u16::from_le_bytes(take(&mut frame)?);
        let address4 = if control.is_four_address() {
            Some(take(&mut frame)?)
        } else {
            None
        };
        let qos = if control.is_qos_data() {
            Some(u16::from_le_bytes(take(&mut frame)?))
        } else {
            None
        };
        let ht_control = if control.has_ht_control() {
            Some(take(&mut frame)?)
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
            body: frame,
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
