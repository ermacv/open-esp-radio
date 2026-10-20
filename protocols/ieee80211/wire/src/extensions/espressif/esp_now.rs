//! Allocation-free ESP-NOW plaintext wire formats.
//!
//! ESP-NOW is carried in a vendor-specific IEEE 802.11 Action management
//! frame. This module owns only the public on-air representation for v1 and
//! plaintext v2: it does not select a radio channel, install a key, allocate a
//! DMA descriptor or weaken a receive filter. In particular, decoding a frame
//! here is not evidence that a backend has admitted or authenticated its
//! sender. Fixed headers are borrowed arrays after a single length check;
//! variable Action data and ciphertext remain borrowed slices.

use crate::frame::FrameControlFields;

use core::fmt;

use crate::sequence::SequenceNumber;

mod v2;

pub use v2::{
    ESP_NOW_V2_ACTION_PREFIX_LEN, ESP_NOW_V2_MAX_ACTION_LEN, ESP_NOW_V2_MAX_ELEMENT_COUNT,
    ESP_NOW_V2_MAX_ELEMENT_PAYLOAD_LEN, ESP_NOW_V2_MAX_MPDU_LEN, ESP_NOW_V2_MAX_PAYLOAD_LEN,
    ESP_NOW_V2_MAX_VENDOR_CONTENT_LEN, ESP_NOW_V2_VERSION, EspNowV2Action, EspNowV2Element,
    EspNowV2Elements, EspNowV2Frame, EspNowV2Payload, EspNowV2WireError,
};

pub const ESP_NOW_ACTION_CATEGORY: u8 = 127;
pub const ESP_NOW_ORGANIZATION_IDENTIFIER: [u8; 3] = [0x18, 0xfe, 0x34];
pub const ESP_NOW_VENDOR_ELEMENT_ID: u8 = 221;
pub const ESP_NOW_VENDOR_ELEMENT_TYPE: u8 = 4;
pub const ESP_NOW_V1_VERSION: u8 = 1;
pub const ESP_NOW_V1_MAX_PAYLOAD_LEN: usize = 250;
pub const ESP_NOW_V1_ACTION_OVERHEAD: usize = 15;
pub const ESP_NOW_MANAGEMENT_HEADER_LEN: usize = 24;
pub const ESP_NOW_V1_MAX_MPDU_LEN: usize =
    ESP_NOW_MANAGEMENT_HEADER_LEN + ESP_NOW_V1_ACTION_OVERHEAD + ESP_NOW_V1_MAX_PAYLOAD_LEN;
/// Generic CCMP header carried between an 802.11 management header and its
/// protected body.
pub const ESP_NOW_CCMP_HEADER_LEN: usize = 8;
/// CCMP MIC length. The protected-envelope parser never treats these bytes as
/// authenticated until a separate cryptographic owner has verified them.
pub const ESP_NOW_CCMP_MIC_LEN: usize = 8;
pub const ESP_NOW_V1_MIN_PROTECTED_MPDU_LEN: usize = ESP_NOW_MANAGEMENT_HEADER_LEN
    + ESP_NOW_CCMP_HEADER_LEN
    + ESP_NOW_V1_ACTION_OVERHEAD
    + ESP_NOW_CCMP_MIC_LEN;
pub const ESP_NOW_V1_MAX_PROTECTED_MPDU_LEN: usize = ESP_NOW_MANAGEMENT_HEADER_LEN
    + ESP_NOW_CCMP_HEADER_LEN
    + ESP_NOW_V1_ACTION_OVERHEAD
    + ESP_NOW_V1_MAX_PAYLOAD_LEN
    + ESP_NOW_CCMP_MIC_LEN;

/// Plaintext wire version selected from the first vendor element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowWireVersion {
    V1,
    V2,
}

/// Failure to classify an ESP-NOW Action body before its version-specific
/// strict parser takes ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowVersionError {
    ActionTooShort { minimum: usize },
    UnsupportedActionCategory(u8),
    InvalidActionOrganizationIdentifier,
    UnsupportedElementId(u8),
    ElementBodyTooShort { declared: u8 },
    InvalidElementOrganizationIdentifier,
    UnsupportedElementType(u8),
    UnsupportedVersion(u8),
}

/// Classify a candidate Action body without weakening either version codec.
///
/// Only the common prefix and first vendor-element header are inspected.
/// Callers must immediately pass the complete MPDU to the selected strict
/// v1/v2 parser; this function is not admission by itself.
pub fn esp_now_wire_version(body: &[u8]) -> Result<EspNowWireVersion, EspNowVersionError> {
    const MINIMUM: usize = ESP_NOW_V1_ACTION_OVERHEAD;
    let mut reader = oer_wire::Reader::new(body);
    let body = reader
        .array::<MINIMUM>()
        .map_err(|_| EspNowVersionError::ActionTooShort { minimum: MINIMUM })?;
    if body[0] != ESP_NOW_ACTION_CATEGORY {
        return Err(EspNowVersionError::UnsupportedActionCategory(body[0]));
    }
    if body[1..4] != ESP_NOW_ORGANIZATION_IDENTIFIER {
        return Err(EspNowVersionError::InvalidActionOrganizationIdentifier);
    }
    if body[8] != ESP_NOW_VENDOR_ELEMENT_ID {
        return Err(EspNowVersionError::UnsupportedElementId(body[8]));
    }
    if body[9] < VENDOR_ELEMENT_FIXED_BODY_LEN as u8 {
        return Err(EspNowVersionError::ElementBodyTooShort { declared: body[9] });
    }
    if body[10..13] != ESP_NOW_ORGANIZATION_IDENTIFIER {
        return Err(EspNowVersionError::InvalidElementOrganizationIdentifier);
    }
    if body[13] != ESP_NOW_VENDOR_ELEMENT_TYPE {
        return Err(EspNowVersionError::UnsupportedElementType(body[13]));
    }
    match EspNowVersion::from_bytes([body[14]]).version() {
        ESP_NOW_V1_VERSION => Ok(EspNowWireVersion::V1),
        ESP_NOW_V2_VERSION => Ok(EspNowWireVersion::V2),
        version => Err(EspNowVersionError::UnsupportedVersion(version)),
    }
}

impl fmt::Display for EspNowVersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ActionTooShort { minimum } => write!(
                formatter,
                "ESP-NOW Action body is shorter than {minimum} bytes"
            ),
            Self::UnsupportedActionCategory(category) => {
                write!(formatter, "unsupported Action category {category}")
            }
            Self::InvalidActionOrganizationIdentifier => {
                formatter.write_str("invalid ESP-NOW Action organization identifier")
            }
            Self::UnsupportedElementId(id) => {
                write!(formatter, "unsupported ESP-NOW element id {id}")
            }
            Self::ElementBodyTooShort { declared } => write!(
                formatter,
                "ESP-NOW vendor element declares only {declared} body bytes"
            ),
            Self::InvalidElementOrganizationIdentifier => {
                formatter.write_str("invalid ESP-NOW element organization identifier")
            }
            Self::UnsupportedElementType(element_type) => {
                write!(formatter, "unsupported ESP-NOW element type {element_type}")
            }
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported ESP-NOW version {version}")
            }
        }
    }
}

impl core::error::Error for EspNowVersionError {}

const ACTION_FRAME_CONTROL: u16 = 0x00d0;
const BROADCAST_ADDRESS: [u8; 6] = [0xff; 6];
const VENDOR_ELEMENT_FIXED_BODY_LEN: usize = 5;
const CCMP_PACKET_NUMBER_MAX: u64 = (1_u64 << 48) - 1;

/// Valid individual address used as the source of an ESP-NOW frame.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct EspNowUnicastAddress([u8; 6]);

impl EspNowUnicastAddress {
    pub const fn new(bytes: [u8; 6]) -> Result<Self, EspNowAddressError> {
        if bytes[0] == 0
            && bytes[1] == 0
            && bytes[2] == 0
            && bytes[3] == 0
            && bytes[4] == 0
            && bytes[5] == 0
        {
            return Err(EspNowAddressError::Unspecified);
        }
        if crate::address::AddressFlags::from_bytes([bytes[0]]).group() {
            return Err(EspNowAddressError::Multicast);
        }
        Ok(Self(bytes))
    }

    pub const fn bytes(self) -> [u8; 6] {
        self.0
    }
}

/// Address accepted as address one of a plaintext ESP-NOW frame.
///
/// Other group addresses are rejected. The initial production profile owns
/// only exact broadcast and individual-peer exchanges.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EspNowDestination {
    Broadcast,
    Unicast(EspNowUnicastAddress),
}

impl EspNowDestination {
    pub const fn new(bytes: [u8; 6]) -> Result<Self, EspNowAddressError> {
        if bytes[0] == 0xff
            && bytes[1] == 0xff
            && bytes[2] == 0xff
            && bytes[3] == 0xff
            && bytes[4] == 0xff
            && bytes[5] == 0xff
        {
            return Ok(Self::Broadcast);
        }
        match EspNowUnicastAddress::new(bytes) {
            Ok(address) => Ok(Self::Unicast(address)),
            Err(error) => Err(error),
        }
    }

    pub const fn bytes(self) -> [u8; 6] {
        match self {
            Self::Broadcast => BROADCAST_ADDRESS,
            Self::Unicast(address) => address.bytes(),
        }
    }

    pub const fn is_broadcast(self) -> bool {
        matches!(self, Self::Broadcast)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowAddressError {
    Unspecified,
    Multicast,
}

impl fmt::Display for EspNowAddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unspecified => "an ESP-NOW address cannot be all zero",
            Self::Multicast => "an ESP-NOW peer must be unicast or the exact broadcast address",
        })
    }
}

impl core::error::Error for EspNowAddressError {}

/// Four opaque anti-replay/anti-collision bytes in the Action frame prefix.
///
/// Entropy ownership stays with the caller. Keeping this value byte-oriented
/// avoids imposing a host-endian interpretation on an on-air opaque field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct EspNowRandomValue([u8; 4]);

impl EspNowRandomValue {
    pub const fn new(bytes: [u8; 4]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 4] {
        self.0
    }
}

/// Nonzero 48-bit packet number observed in a protected ESP-NOW envelope.
///
/// This is wire metadata only. Possessing it does not prove that the CCMP MIC
/// was checked or that the frame is fresh.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct EspNowCcmpPacketNumber(u64);

impl EspNowCcmpPacketNumber {
    pub const fn new(value: u64) -> Result<Self, EspNowCcmpPacketNumberError> {
        if value == 0 {
            return Err(EspNowCcmpPacketNumberError::Zero);
        }
        if value > CCMP_PACKET_NUMBER_MAX {
            return Err(EspNowCcmpPacketNumberError::Exceeds48Bits(value));
        }
        Ok(Self(value))
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn bytes(self) -> [u8; 6] {
        let bytes = self.0.to_le_bytes();
        [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowCcmpPacketNumberError {
    Zero,
    Exceeds48Bits(u64),
}

impl fmt::Display for EspNowCcmpPacketNumberError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => formatter.write_str("a CCMP packet number must be nonzero"),
            Self::Exceeds48Bits(value) => {
                write!(formatter, "CCMP packet number {value} exceeds 48 bits")
            }
        }
    }
}

impl core::error::Error for EspNowCcmpPacketNumberError {}

/// Borrowed v1 payload whose size fits the one-byte vendor-element length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct EspNowV1Payload<'payload>(&'payload [u8]);

impl<'payload> EspNowV1Payload<'payload> {
    pub const fn new(bytes: &'payload [u8]) -> Result<Self, EspNowV1WireError> {
        if bytes.len() > ESP_NOW_V1_MAX_PAYLOAD_LEN {
            return Err(EspNowV1WireError::PayloadTooLong {
                length: bytes.len(),
            });
        }
        Ok(Self(bytes))
    }

    pub const fn bytes(self) -> &'payload [u8] {
        self.0
    }

    pub const fn len(self) -> usize {
        self.0.len()
    }

    pub const fn is_empty(self) -> bool {
        self.0.is_empty()
    }
}

/// Strictly validated ESP-NOW v1 vendor Action body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowV1Action<'payload> {
    random_value: EspNowRandomValue,
    payload: EspNowV1Payload<'payload>,
}

impl<'payload> EspNowV1Action<'payload> {
    pub const fn new(
        random_value: EspNowRandomValue,
        payload: &'payload [u8],
    ) -> Result<Self, EspNowV1WireError> {
        let payload = match EspNowV1Payload::new(payload) {
            Ok(payload) => payload,
            Err(error) => return Err(error),
        };
        Ok(Self {
            random_value,
            payload,
        })
    }

    pub const fn random_value(self) -> EspNowRandomValue {
        self.random_value
    }

    pub const fn payload(self) -> EspNowV1Payload<'payload> {
        self.payload
    }

    pub const fn encoded_len(self) -> usize {
        ESP_NOW_V1_ACTION_OVERHEAD + self.payload.len()
    }

    /// Encode category, action OUI, random value and exactly one v1 vendor IE.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, EspNowV1WireError> {
        let required = self.encoded_len();
        if output.len() < required {
            return Err(EspNowV1WireError::OutputTooSmall { required });
        }

        let output = &mut output[..required];
        output[0] = ESP_NOW_ACTION_CATEGORY;
        output[1..4].copy_from_slice(&ESP_NOW_ORGANIZATION_IDENTIFIER);
        output[4..8].copy_from_slice(&self.random_value.bytes());
        output[8] = ESP_NOW_VENDOR_ELEMENT_ID;
        output[9] = (VENDOR_ELEMENT_FIXED_BODY_LEN + self.payload.len()) as u8;
        output[10..13].copy_from_slice(&ESP_NOW_ORGANIZATION_IDENTIFIER);
        output[13] = ESP_NOW_VENDOR_ELEMENT_TYPE;
        // Version occupies the low nibble. All v1 reserved bits remain zero.
        output[14] = ESP_NOW_V1_VERSION;
        output[15..].copy_from_slice(self.payload.bytes());
        Ok(required)
    }

    /// Parse one complete v1 Action body and reject trailing or partial IEs.
    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    pub fn parse(input: &'payload [u8]) -> Result<Self, EspNowV1WireError> {
        let mut reader = oer_wire::Reader::new(input);
        let Ok(
            &[
                category,
                a0,
                a1,
                a2,
                r0,
                r1,
                r2,
                r3,
                element_id,
                element_length,
            ],
        ) = reader.array::<10>()
        else {
            return Err(EspNowV1WireError::FrameTooShort { minimum: 10 });
        };
        let element_body = reader.remaining();
        if category != ESP_NOW_ACTION_CATEGORY {
            return Err(EspNowV1WireError::UnsupportedActionCategory(category));
        }
        if [a0, a1, a2] != ESP_NOW_ORGANIZATION_IDENTIFIER {
            return Err(EspNowV1WireError::InvalidActionOrganizationIdentifier);
        }
        if element_id != ESP_NOW_VENDOR_ELEMENT_ID {
            return Err(EspNowV1WireError::UnsupportedElementId(element_id));
        }

        let declared = usize::from(element_length);
        if declared < VENDOR_ELEMENT_FIXED_BODY_LEN {
            return Err(EspNowV1WireError::ElementBodyTooShort {
                declared: element_length,
            });
        }
        if element_body.len() != declared {
            return Err(EspNowV1WireError::ElementLengthMismatch {
                declared: element_length,
                actual: element_body.len(),
            });
        }
        let mut reader = oer_wire::Reader::new(element_body);
        let Ok(&[o0, o1, o2, element_type, version]) =
            reader.array::<VENDOR_ELEMENT_FIXED_BODY_LEN>()
        else {
            return Err(EspNowV1WireError::ElementBodyTooShort {
                declared: element_length,
            });
        };
        let payload = reader.remaining();
        if [o0, o1, o2] != ESP_NOW_ORGANIZATION_IDENTIFIER {
            return Err(EspNowV1WireError::InvalidElementOrganizationIdentifier);
        }
        if element_type != ESP_NOW_VENDOR_ELEMENT_TYPE {
            return Err(EspNowV1WireError::UnsupportedElementType(element_type));
        }
        if EspNowVersion::from_bytes([version]).upper() != 0 {
            return Err(EspNowV1WireError::ReservedVersionBitsSet(version));
        }
        if EspNowVersion::from_bytes([version]).version() != ESP_NOW_V1_VERSION {
            return Err(EspNowV1WireError::UnsupportedVersion(
                EspNowVersion::from_bytes([version]).version(),
            ));
        }

        Ok(Self {
            random_value: EspNowRandomValue::new([r0, r1, r2, r3]),
            payload: EspNowV1Payload(payload),
        })
    }
}

/// Complete plaintext ESP-NOW v1 MPDU without the hardware-generated FCS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowV1Frame<'payload> {
    destination: EspNowDestination,
    source: EspNowUnicastAddress,
    sequence_number: SequenceNumber,
    retry: bool,
    action: EspNowV1Action<'payload>,
}

impl<'payload> EspNowV1Frame<'payload> {
    pub const fn new(
        destination: EspNowDestination,
        source: EspNowUnicastAddress,
        sequence_number: SequenceNumber,
        random_value: EspNowRandomValue,
        payload: &'payload [u8],
    ) -> Result<Self, EspNowV1WireError> {
        let action = match EspNowV1Action::new(random_value, payload) {
            Ok(action) => action,
            Err(error) => return Err(error),
        };
        Ok(Self {
            destination,
            source,
            sequence_number,
            retry: false,
            action,
        })
    }

    pub const fn destination(self) -> EspNowDestination {
        self.destination
    }

    pub const fn source(self) -> EspNowUnicastAddress {
        self.source
    }

    pub const fn sequence_number(self) -> SequenceNumber {
        self.sequence_number
    }

    pub const fn retry(self) -> bool {
        self.retry
    }

    pub const fn action(self) -> EspNowV1Action<'payload> {
        self.action
    }

    pub const fn encoded_len(self) -> usize {
        ESP_NOW_MANAGEMENT_HEADER_LEN + self.action.encoded_len()
    }

    /// Encode an Action MPDU with broadcast BSSID and no FCS.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, EspNowV1WireError> {
        let required = self.encoded_len();
        if output.len() < required {
            return Err(EspNowV1WireError::OutputTooSmall { required });
        }

        let output = &mut output[..required];
        output[..ESP_NOW_MANAGEMENT_HEADER_LEN].fill(0);
        let frame_control = FrameControlFields::from_raw(ACTION_FRAME_CONTROL)
            .with_retry(self.retry)
            .raw();
        output[0..2].copy_from_slice(&frame_control.to_le_bytes());
        output[4..10].copy_from_slice(&self.destination.bytes());
        output[10..16].copy_from_slice(&self.source.bytes());
        output[16..22].copy_from_slice(&BROADCAST_ADDRESS);
        output[22..24].copy_from_slice(&self.sequence_number.sequence_control().to_le_bytes());
        self.action
            .encode(&mut output[ESP_NOW_MANAGEMENT_HEADER_LEN..])?;
        Ok(required)
    }

    /// Parse one complete unprotected, unfragmented ESP-NOW v1 Action MPDU.
    pub fn parse(input: &'payload [u8]) -> Result<Self, EspNowV1WireError> {
        let mut reader = oer_wire::Reader::new(input);
        let header = reader
            .array::<ESP_NOW_MANAGEMENT_HEADER_LEN>()
            .map_err(|_| EspNowV1WireError::FrameTooShort {
                minimum: ESP_NOW_MANAGEMENT_HEADER_LEN,
            })?;
        let action = reader.remaining();
        let frame_control = u16::from_le_bytes([header[0], header[1]]);
        if FrameControlFields::from_raw(frame_control).version() != 0
            || FrameControlFields::from_raw(frame_control).type_and_subtype()
                != FrameControlFields::from_raw(ACTION_FRAME_CONTROL).type_and_subtype()
            || FrameControlFields::from_raw(frame_control).ds() != 0
            || (FrameControlFields::from_raw(frame_control).power_management()
                || FrameControlFields::from_raw(frame_control).more_data()
                || FrameControlFields::from_raw(frame_control).order())
        {
            return Err(EspNowV1WireError::UnsupportedFrameControl(frame_control));
        }
        if FrameControlFields::from_raw(frame_control).protected() {
            return Err(EspNowV1WireError::ProtectedFrameUnsupported);
        }

        let destination = EspNowDestination::new([
            header[4], header[5], header[6], header[7], header[8], header[9],
        ])
        .map_err(EspNowV1WireError::InvalidDestination)?;
        let source = EspNowUnicastAddress::new([
            header[10], header[11], header[12], header[13], header[14], header[15],
        ])
        .map_err(EspNowV1WireError::InvalidSource)?;
        if header[16..22] != BROADCAST_ADDRESS {
            return Err(EspNowV1WireError::InvalidBssid);
        }
        let sequence_control = u16::from_le_bytes([header[22], header[23]]);
        if FrameControlFields::from_raw(frame_control).more_fragments()
            || crate::sequence::SequenceControl::from_bytes((sequence_control).to_le_bytes())
                .fragment()
                != 0
        {
            return Err(EspNowV1WireError::FragmentedFrame(
                crate::sequence::SequenceControl::from_bytes((sequence_control).to_le_bytes())
                    .fragment(),
            ));
        }
        let action = EspNowV1Action::parse(action)?;
        Ok(Self {
            destination,
            source,
            sequence_number: SequenceNumber::from_sequence_control(sequence_control),
            retry: FrameControlFields::from_raw(frame_control).retry(),
            action,
        })
    }
}

/// Strictly parsed outer metadata of one encrypted, unicast ESP-NOW v1 MPDU.
///
/// The Action body is intentionally opaque ciphertext. Parsing this envelope
/// proves only structural admission: it does not authenticate, decrypt or
/// interpret the body, and it does not advance a replay window. The exact
/// ESP-NOW Action-frame AAD and authenticated transform contract are not present in the
/// reviewed evidence, so no encrypted encoder/decoder is exposed here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowProtectedV1Envelope<'frame> {
    management_header: &'frame [u8; ESP_NOW_MANAGEMENT_HEADER_LEN],
    destination: EspNowUnicastAddress,
    source: EspNowUnicastAddress,
    sequence_number: SequenceNumber,
    retry: bool,
    key_id: u8,
    packet_number: EspNowCcmpPacketNumber,
    ccmp_header: &'frame [u8; ESP_NOW_CCMP_HEADER_LEN],
    ciphertext: &'frame [u8],
    mic: &'frame [u8; ESP_NOW_CCMP_MIC_LEN],
}

impl<'frame> EspNowProtectedV1Envelope<'frame> {
    /// Parse protected-frame metadata while leaving ciphertext and MIC
    /// unauthenticated and uninterpreted.
    pub fn parse(input: &'frame [u8]) -> Result<Self, EspNowProtectedV1WireError> {
        let too_short = || EspNowProtectedV1WireError::FrameTooShort {
            minimum: ESP_NOW_V1_MIN_PROTECTED_MPDU_LEN,
        };
        let mut reader = oer_wire::Reader::new(input);
        let management_header = reader
            .array::<ESP_NOW_MANAGEMENT_HEADER_LEN>()
            .map_err(|_| too_short())?;
        let ccmp_header = reader
            .array::<ESP_NOW_CCMP_HEADER_LEN>()
            .map_err(|_| too_short())?;
        let protected_body = reader.remaining();
        let (ciphertext, mic) = protected_body
            .split_last_chunk::<ESP_NOW_CCMP_MIC_LEN>()
            .ok_or_else(too_short)?;
        if ciphertext.len() < ESP_NOW_V1_ACTION_OVERHEAD {
            return Err(too_short());
        }
        if input.len() > ESP_NOW_V1_MAX_PROTECTED_MPDU_LEN {
            return Err(EspNowProtectedV1WireError::FrameTooLong {
                maximum: ESP_NOW_V1_MAX_PROTECTED_MPDU_LEN,
                actual: input.len(),
            });
        }

        let frame_control = u16::from_le_bytes([management_header[0], management_header[1]]);
        if FrameControlFields::from_raw(frame_control).version() != 0
            || FrameControlFields::from_raw(frame_control).type_and_subtype()
                != FrameControlFields::from_raw(ACTION_FRAME_CONTROL).type_and_subtype()
            || FrameControlFields::from_raw(frame_control).ds() != 0
            || (FrameControlFields::from_raw(frame_control).power_management()
                || FrameControlFields::from_raw(frame_control).more_data()
                || FrameControlFields::from_raw(frame_control).order())
        {
            return Err(EspNowProtectedV1WireError::UnsupportedFrameControl(
                frame_control,
            ));
        }
        if !FrameControlFields::from_raw(frame_control).protected() {
            return Err(EspNowProtectedV1WireError::ProtectionRequired);
        }

        let destination_bytes = [
            management_header[4],
            management_header[5],
            management_header[6],
            management_header[7],
            management_header[8],
            management_header[9],
        ];
        if destination_bytes == BROADCAST_ADDRESS {
            return Err(EspNowProtectedV1WireError::EncryptedBroadcastUnsupported);
        }
        let destination = EspNowUnicastAddress::new(destination_bytes)
            .map_err(EspNowProtectedV1WireError::InvalidDestination)?;
        let source = EspNowUnicastAddress::new([
            management_header[10],
            management_header[11],
            management_header[12],
            management_header[13],
            management_header[14],
            management_header[15],
        ])
        .map_err(EspNowProtectedV1WireError::InvalidSource)?;
        if management_header[16..22] != BROADCAST_ADDRESS {
            return Err(EspNowProtectedV1WireError::InvalidBssid);
        }

        let sequence_control = u16::from_le_bytes([management_header[22], management_header[23]]);
        if FrameControlFields::from_raw(frame_control).more_fragments()
            || crate::sequence::SequenceControl::from_bytes((sequence_control).to_le_bytes())
                .fragment()
                != 0
        {
            return Err(EspNowProtectedV1WireError::FragmentedFrame(
                crate::sequence::SequenceControl::from_bytes((sequence_control).to_le_bytes())
                    .fragment(),
            ));
        }

        if ccmp_header[2] != 0
            || crate::ccmp::CcmpKeyControl::from_bytes([ccmp_header[3]]).reserved() != 0
        {
            return Err(EspNowProtectedV1WireError::ReservedCcmpBitsSet {
                reserved_byte: ccmp_header[2],
                control: ccmp_header[3],
            });
        }
        if !crate::ccmp::CcmpKeyControl::from_bytes([ccmp_header[3]]).extended_iv() {
            return Err(EspNowProtectedV1WireError::MissingExtendedIv);
        }
        let packet_number = crate::ccmp::CcmpPacketNumber::from_header_bytes(*ccmp_header).value();
        let packet_number = EspNowCcmpPacketNumber::new(packet_number)
            .map_err(EspNowProtectedV1WireError::InvalidPacketNumber)?;

        Ok(Self {
            management_header,
            destination,
            source,
            sequence_number: SequenceNumber::from_sequence_control(sequence_control),
            retry: FrameControlFields::from_raw(frame_control).retry(),
            key_id: crate::ccmp::CcmpKeyControl::from_bytes([ccmp_header[3]]).key_id(),
            packet_number,
            ccmp_header,
            ciphertext,
            mic,
        })
    }

    pub const fn management_header(self) -> &'frame [u8; ESP_NOW_MANAGEMENT_HEADER_LEN] {
        self.management_header
    }

    pub const fn destination(self) -> EspNowUnicastAddress {
        self.destination
    }

    pub const fn source(self) -> EspNowUnicastAddress {
        self.source
    }

    pub const fn sequence_number(self) -> SequenceNumber {
        self.sequence_number
    }

    pub const fn retry(self) -> bool {
        self.retry
    }

    pub const fn key_id(self) -> u8 {
        self.key_id
    }

    pub const fn packet_number(self) -> EspNowCcmpPacketNumber {
        self.packet_number
    }

    pub const fn ccmp_header(self) -> &'frame [u8; ESP_NOW_CCMP_HEADER_LEN] {
        self.ccmp_header
    }

    pub const fn ciphertext(self) -> &'frame [u8] {
        self.ciphertext
    }

    pub const fn mic(self) -> &'frame [u8; ESP_NOW_CCMP_MIC_LEN] {
        self.mic
    }
}

/// Structural failures at the protected-envelope boundary. None of these
/// outcomes make an authentication claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowProtectedV1WireError {
    FrameTooShort { minimum: usize },
    FrameTooLong { maximum: usize, actual: usize },
    UnsupportedFrameControl(u16),
    ProtectionRequired,
    EncryptedBroadcastUnsupported,
    InvalidDestination(EspNowAddressError),
    InvalidSource(EspNowAddressError),
    InvalidBssid,
    FragmentedFrame(u8),
    ReservedCcmpBitsSet { reserved_byte: u8, control: u8 },
    MissingExtendedIv,
    InvalidPacketNumber(EspNowCcmpPacketNumberError),
}

impl fmt::Display for EspNowProtectedV1WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameTooShort { minimum } => {
                write!(
                    formatter,
                    "protected ESP-NOW input is shorter than {minimum} bytes"
                )
            }
            Self::FrameTooLong { maximum, actual } => write!(
                formatter,
                "protected ESP-NOW input has {actual} bytes, exceeding {maximum}"
            ),
            Self::UnsupportedFrameControl(frame_control) => write!(
                formatter,
                "unsupported protected ESP-NOW frame-control value {frame_control:#06x}"
            ),
            Self::ProtectionRequired => {
                formatter.write_str("protected ESP-NOW envelope requires the Protected flag")
            }
            Self::EncryptedBroadcastUnsupported => formatter
                .write_str("ESP-NOW encryption is supported only for an individual destination"),
            Self::InvalidDestination(error) => write!(formatter, "invalid destination: {error}"),
            Self::InvalidSource(error) => write!(formatter, "invalid source: {error}"),
            Self::InvalidBssid => formatter.write_str("protected ESP-NOW BSSID is not broadcast"),
            Self::FragmentedFrame(fragment) => {
                write!(
                    formatter,
                    "protected ESP-NOW fragment {fragment} is unsupported"
                )
            }
            Self::ReservedCcmpBitsSet {
                reserved_byte,
                control,
            } => write!(
                formatter,
                "protected ESP-NOW CCMP header has reserved bits set ({reserved_byte:#04x}, {control:#04x})"
            ),
            Self::MissingExtendedIv => {
                formatter.write_str("protected ESP-NOW CCMP header is missing ExtIV")
            }
            Self::InvalidPacketNumber(error) => {
                write!(
                    formatter,
                    "invalid protected ESP-NOW packet number: {error}"
                )
            }
        }
    }
}

impl core::error::Error for EspNowProtectedV1WireError {}

/// First unavailable stage of the interoperable encrypted-v1 codec.
///
/// The outer envelope is parseable, but constructing or decrypting the Action
/// body would require an exact, reviewed CCMP AAD and callback contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowEncryptedV1Unavailable {
    ActionAadContractUnproven,
}

impl fmt::Display for EspNowEncryptedV1Unavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "encrypted ESP-NOW v1 Action AAD and construct/decrypt contract are unproven",
        )
    }
}

impl core::error::Error for EspNowEncryptedV1Unavailable {}

/// Why bytes cannot cross the strict plaintext ESP-NOW v1 boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowV1WireError {
    PayloadTooLong { length: usize },
    OutputTooSmall { required: usize },
    FrameTooShort { minimum: usize },
    InvalidDestination(EspNowAddressError),
    InvalidSource(EspNowAddressError),
    InvalidBssid,
    UnsupportedFrameControl(u16),
    ProtectedFrameUnsupported,
    FragmentedFrame(u8),
    UnsupportedActionCategory(u8),
    InvalidActionOrganizationIdentifier,
    UnsupportedElementId(u8),
    ElementBodyTooShort { declared: u8 },
    ElementLengthMismatch { declared: u8, actual: usize },
    InvalidElementOrganizationIdentifier,
    UnsupportedElementType(u8),
    ReservedVersionBitsSet(u8),
    UnsupportedVersion(u8),
}

impl fmt::Display for EspNowV1WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLong { length } => write!(
                formatter,
                "ESP-NOW v1 payload length {length} exceeds {ESP_NOW_V1_MAX_PAYLOAD_LEN} bytes"
            ),
            Self::OutputTooSmall { required } => {
                write!(formatter, "ESP-NOW output needs {required} bytes")
            }
            Self::FrameTooShort { minimum } => {
                write!(formatter, "ESP-NOW input is shorter than {minimum} bytes")
            }
            Self::InvalidDestination(error) => write!(formatter, "invalid destination: {error}"),
            Self::InvalidSource(error) => write!(formatter, "invalid source: {error}"),
            Self::InvalidBssid => formatter.write_str("ESP-NOW v1 BSSID is not broadcast"),
            Self::UnsupportedFrameControl(frame_control) => write!(
                formatter,
                "unsupported ESP-NOW frame-control value {frame_control:#06x}"
            ),
            Self::ProtectedFrameUnsupported => {
                formatter.write_str("protected ESP-NOW frames are outside the v1 plaintext profile")
            }
            Self::FragmentedFrame(fragment) => {
                write!(formatter, "ESP-NOW fragment {fragment} is unsupported")
            }
            Self::UnsupportedActionCategory(category) => {
                write!(formatter, "unsupported Action category {category}")
            }
            Self::InvalidActionOrganizationIdentifier => {
                formatter.write_str("invalid ESP-NOW Action organization identifier")
            }
            Self::UnsupportedElementId(id) => {
                write!(formatter, "unsupported ESP-NOW element id {id}")
            }
            Self::ElementBodyTooShort { declared } => write!(
                formatter,
                "ESP-NOW vendor element declares only {declared} body bytes"
            ),
            Self::ElementLengthMismatch { declared, actual } => write!(
                formatter,
                "ESP-NOW vendor element declares {declared} bytes but contains {actual}"
            ),
            Self::InvalidElementOrganizationIdentifier => {
                formatter.write_str("invalid ESP-NOW element organization identifier")
            }
            Self::UnsupportedElementType(element_type) => {
                write!(formatter, "unsupported ESP-NOW element type {element_type}")
            }
            Self::ReservedVersionBitsSet(version) => write!(
                formatter,
                "ESP-NOW v1 version byte {version:#04x} has reserved bits set"
            ),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported ESP-NOW version {version}")
            }
        }
    }
}

impl core::error::Error for EspNowV1WireError {}

oer_wire::bitfield! {
    /// SOURCE: Espressif ESP-NOW specification, Frame Format (vendor-specific content).
    pub struct EspNowVersion[1] (little) {
        /// SOURCE: Espressif ESP-NOW specification, Frame Format (vendor-specific content).
        pub version, with_version: u8 = 0..4;
        /// SOURCE: Espressif ESP-NOW specification, Frame Format (vendor-specific content).
        pub upper, with_upper: u8 = 4..8;
        /// SOURCE: Espressif ESP-NOW specification, Frame Format (vendor-specific content).
        pub more_data, with_more_data: bool = 4..5;
        /// SOURCE: Espressif ESP-NOW specification, Frame Format (vendor-specific content).
        pub reserved_v2, with_reserved_v2: u8 = 5..8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> EspNowUnicastAddress {
        EspNowUnicastAddress::new([2, 0, 0, 0, 0, 1]).unwrap()
    }

    #[test]
    fn every_prefix_of_a_v1_frame_is_handled() {
        let frame = EspNowV1Frame::new(
            EspNowDestination::Broadcast,
            source(),
            SequenceNumber::from_sequence_control(0),
            EspNowRandomValue::new([1, 2, 3, 4]),
            &[7; 9],
        )
        .unwrap();
        let mut bytes = [0_u8; 64];
        let len = frame.encode(&mut bytes).unwrap();
        for prefix in 0..len {
            assert!(EspNowV1Frame::parse(&bytes[..prefix]).is_err());
            let _ = EspNowV1Action::parse(&bytes[..prefix]);
        }
        assert!(EspNowV1Frame::parse(&bytes[..len]).is_ok());
    }

    #[test]
    fn every_prefix_of_a_v2_frame_is_handled() {
        let payload = [9_u8; 300];
        let frame = EspNowV2Frame::new(
            EspNowDestination::Broadcast,
            source(),
            SequenceNumber::from_sequence_control(0),
            EspNowRandomValue::new([1, 2, 3, 4]),
            &payload,
        )
        .unwrap();
        let mut bytes = [0_u8; 400];
        let len = frame.encode(&mut bytes).unwrap();
        for prefix in 0..len {
            assert!(EspNowV2Frame::parse(&bytes[..prefix]).is_err());
            let _ = EspNowV2Action::parse(&bytes[..prefix]);
        }
        let parsed = EspNowV2Frame::parse(&bytes[..len]).unwrap();
        let mut copied = [0_u8; 300];
        assert_eq!(parsed.action().copy_payload(&mut copied), Ok(300));
        assert_eq!(copied, payload);
        let mut small = [0_u8; 299];
        assert!(parsed.action().copy_payload(&mut small).is_err());
    }
}
