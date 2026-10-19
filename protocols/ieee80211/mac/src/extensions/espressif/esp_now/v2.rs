//! Strict, allocation-free ESP-NOW v2 plaintext framing.
//!
//! The public ESP-NOW v2 format carries one to six vendor-specific elements
//! in a single Action MPDU. This is application-level element fragmentation,
//! not IEEE 802.11 MAC fragmentation. The encoder uses a deterministic
//! 250-byte split; the parser accepts every split allowed by the public format
//! and validates the complete `More data` chain before exposing any element.

use core::fmt;

use crate::sequence::SequenceNumber;

mod reassembly;
pub use reassembly::EspNowV2Reassembly;

use super::{
    ACTION_FRAME_CONTROL, BROADCAST_ADDRESS, ESP_NOW_ACTION_CATEGORY,
    ESP_NOW_MANAGEMENT_HEADER_LEN, ESP_NOW_ORGANIZATION_IDENTIFIER, ESP_NOW_VENDOR_ELEMENT_ID,
    ESP_NOW_VENDOR_ELEMENT_TYPE, EspNowAddressError, EspNowDestination, EspNowRandomValue,
    EspNowUnicastAddress, FRAME_TYPE_AND_SUBTYPE_MASK, MORE_DATA_FLAG, MORE_FRAGMENTS_FLAG,
    ORDER_FLAG, POWER_MANAGEMENT_FLAG, PROTECTED_FLAG, PROTOCOL_VERSION_MASK, RETRY_FLAG,
    TO_FROM_DS_MASK, VENDOR_ELEMENT_FIXED_BODY_LEN,
};

/// ESP-NOW version encoded in the low nibble of every v2 vendor element.
pub const ESP_NOW_V2_VERSION: u8 = 2;
/// Maximum application bytes carried by one v2 vendor element.
pub const ESP_NOW_V2_MAX_ELEMENT_PAYLOAD_LEN: usize = 250;
/// Maximum number of vendor elements in one public ESP-NOW v2 datagram.
pub const ESP_NOW_V2_MAX_ELEMENT_COUNT: usize = 6;
/// Maximum application payload documented for ESP-NOW v2.
pub const ESP_NOW_V2_MAX_PAYLOAD_LEN: usize = 1470;
/// Action prefix: category, organization identifier and random value.
pub const ESP_NOW_V2_ACTION_PREFIX_LEN: usize = 8;
/// Maximum encoded bytes occupied by the repeated vendor elements.
pub const ESP_NOW_V2_MAX_VENDOR_CONTENT_LEN: usize =
    ESP_NOW_V2_MAX_PAYLOAD_LEN + ESP_NOW_V2_MAX_ELEMENT_COUNT * VENDOR_ELEMENT_ENCODED_OVERHEAD;
/// Maximum plaintext v2 Action body, excluding the management header and FCS.
pub const ESP_NOW_V2_MAX_ACTION_LEN: usize =
    ESP_NOW_V2_ACTION_PREFIX_LEN + ESP_NOW_V2_MAX_VENDOR_CONTENT_LEN;
/// Maximum plaintext v2 MPDU without the hardware-generated FCS.
pub const ESP_NOW_V2_MAX_MPDU_LEN: usize =
    ESP_NOW_MANAGEMENT_HEADER_LEN + ESP_NOW_V2_MAX_ACTION_LEN;

const VENDOR_ELEMENT_ENCODED_OVERHEAD: usize = 2 + VENDOR_ELEMENT_FIXED_BODY_LEN;
const V2_MORE_DATA_FLAG: u8 = 0x10;
const V2_RESERVED_MASK: u8 = 0xe0;
const VERSION_MASK: u8 = 0x0f;
const MIN_ACTION_LEN: usize = ESP_NOW_V2_ACTION_PREFIX_LEN + VENDOR_ELEMENT_ENCODED_OVERHEAD;

/// Borrowed v2 payload validated against the public 1470-byte limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct EspNowV2Payload<'payload>(&'payload [u8]);

impl<'payload> EspNowV2Payload<'payload> {
    pub const fn new(bytes: &'payload [u8]) -> Result<Self, EspNowV2WireError> {
        if bytes.len() > ESP_NOW_V2_MAX_PAYLOAD_LEN {
            return Err(EspNowV2WireError::PayloadTooLong {
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

#[derive(Clone, Copy, Debug)]
enum EspNowV2ActionContent<'payload> {
    Contiguous(EspNowV2Payload<'payload>),
    EncodedElements(&'payload [u8]),
}

/// One validated element from an ESP-NOW v2 Action body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowV2Element<'payload> {
    payload: &'payload [u8],
    more_data: bool,
}

impl<'payload> EspNowV2Element<'payload> {
    pub const fn payload(self) -> &'payload [u8] {
        self.payload
    }

    pub const fn more_data(self) -> bool {
        self.more_data
    }
}

#[derive(Clone, Copy, Debug)]
enum EspNowV2ElementSource<'payload> {
    Contiguous(&'payload [u8]),
    Encoded(&'payload [u8]),
}

/// Exact-size iterator over already-validated v2 element bodies.
#[derive(Clone, Copy, Debug)]
pub struct EspNowV2Elements<'payload> {
    source: EspNowV2ElementSource<'payload>,
    remaining_elements: usize,
}

impl<'payload> Iterator for EspNowV2Elements<'payload> {
    type Item = EspNowV2Element<'payload>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining_elements == 0 {
            return None;
        }

        let more_data = self.remaining_elements > 1;
        let element = match self.source {
            EspNowV2ElementSource::Contiguous(remaining) => {
                let body_length = remaining.len().min(ESP_NOW_V2_MAX_ELEMENT_PAYLOAD_LEN);
                let (payload, tail) = remaining.split_at(body_length);
                self.source = EspNowV2ElementSource::Contiguous(tail);
                EspNowV2Element { payload, more_data }
            }
            EspNowV2ElementSource::Encoded(remaining) => {
                // Construction of this iterator is private and follows a full
                // validation pass, so every fixed and declared extent exists.
                let declared = usize::from(remaining[1]);
                let encoded_length = 2 + declared;
                let payload = &remaining[VENDOR_ELEMENT_ENCODED_OVERHEAD..encoded_length];
                let more_data = remaining[6] & V2_MORE_DATA_FLAG != 0;
                self.source = EspNowV2ElementSource::Encoded(&remaining[encoded_length..]);
                EspNowV2Element { payload, more_data }
            }
        };
        self.remaining_elements -= 1;
        Some(element)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining_elements, Some(self.remaining_elements))
    }
}

impl ExactSizeIterator for EspNowV2Elements<'_> {}
impl core::iter::FusedIterator for EspNowV2Elements<'_> {}

/// Strictly validated ESP-NOW v2 vendor Action body.
///
/// A newly constructed value borrows one contiguous application payload. A
/// parsed value borrows the encoded vendor elements. Both representations are
/// exposed uniformly through [`Self::elements`] and can be copied into a
/// caller-owned reassembly buffer without allocation.
#[derive(Clone, Copy, Debug)]
pub struct EspNowV2Action<'payload> {
    random_value: EspNowRandomValue,
    content: EspNowV2ActionContent<'payload>,
    payload_length: usize,
    element_count: usize,
    vendor_content_length: usize,
}

impl<'payload> EspNowV2Action<'payload> {
    pub const fn new(
        random_value: EspNowRandomValue,
        payload: &'payload [u8],
    ) -> Result<Self, EspNowV2WireError> {
        let payload = match EspNowV2Payload::new(payload) {
            Ok(payload) => payload,
            Err(error) => return Err(error),
        };
        let element_count = element_count_for_payload(payload.len());
        Ok(Self {
            random_value,
            content: EspNowV2ActionContent::Contiguous(payload),
            payload_length: payload.len(),
            element_count,
            vendor_content_length: payload.len() + element_count * VENDOR_ELEMENT_ENCODED_OVERHEAD,
        })
    }

    pub const fn random_value(self) -> EspNowRandomValue {
        self.random_value
    }

    pub const fn payload_len(self) -> usize {
        self.payload_length
    }

    pub const fn is_empty(self) -> bool {
        self.payload_length == 0
    }

    pub const fn element_count(self) -> usize {
        self.element_count
    }

    pub const fn encoded_len(self) -> usize {
        ESP_NOW_V2_ACTION_PREFIX_LEN + self.vendor_content_length
    }

    pub const fn elements(self) -> EspNowV2Elements<'payload> {
        let source = match self.content {
            EspNowV2ActionContent::Contiguous(payload) => {
                EspNowV2ElementSource::Contiguous(payload.bytes())
            }
            EspNowV2ActionContent::EncodedElements(elements) => {
                EspNowV2ElementSource::Encoded(elements)
            }
        };
        EspNowV2Elements {
            source,
            remaining_elements: self.element_count,
        }
    }

    /// Reassemble into caller storage after checking capacity up front.
    ///
    /// On a capacity failure `output` is left unchanged.
    pub fn copy_payload(self, output: &mut [u8]) -> Result<usize, EspNowV2WireError> {
        if output.len() < self.payload_length {
            return Err(EspNowV2WireError::OutputTooSmall {
                required: self.payload_length,
            });
        }
        let mut offset = 0;
        for element in self.elements() {
            let end = offset + element.payload.len();
            output[offset..end].copy_from_slice(element.payload);
            offset = end;
        }
        Ok(offset)
    }

    /// Encode category, Action OUI, random value and the complete v2 element
    /// chain. Constructed values use a deterministic greedy 250-byte split.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, EspNowV2WireError> {
        let required = self.encoded_len();
        if output.len() < required {
            return Err(EspNowV2WireError::OutputTooSmall { required });
        }

        let output = &mut output[..required];
        output[0] = ESP_NOW_ACTION_CATEGORY;
        output[1..4].copy_from_slice(&ESP_NOW_ORGANIZATION_IDENTIFIER);
        output[4..8].copy_from_slice(&self.random_value.bytes());

        let mut offset = ESP_NOW_V2_ACTION_PREFIX_LEN;
        for element in self.elements() {
            let payload = element.payload();
            let encoded_length = VENDOR_ELEMENT_ENCODED_OVERHEAD + payload.len();
            output[offset] = ESP_NOW_VENDOR_ELEMENT_ID;
            output[offset + 1] = (VENDOR_ELEMENT_FIXED_BODY_LEN + payload.len()) as u8;
            output[offset + 2..offset + 5].copy_from_slice(&ESP_NOW_ORGANIZATION_IDENTIFIER);
            output[offset + 5] = ESP_NOW_VENDOR_ELEMENT_TYPE;
            output[offset + 6] = ESP_NOW_V2_VERSION
                | if element.more_data() {
                    V2_MORE_DATA_FLAG
                } else {
                    0
                };
            output[offset + VENDOR_ELEMENT_ENCODED_OVERHEAD..offset + encoded_length]
                .copy_from_slice(payload);
            offset += encoded_length;
        }
        Ok(required)
    }

    /// Parse one complete v2 Action body and validate every element before
    /// returning an iterator over any application bytes.
    pub fn parse(input: &'payload [u8]) -> Result<Self, EspNowV2WireError> {
        if input.len() < MIN_ACTION_LEN {
            return Err(EspNowV2WireError::FrameTooShort {
                minimum: MIN_ACTION_LEN,
            });
        }
        if input.len() > ESP_NOW_V2_MAX_ACTION_LEN {
            return Err(EspNowV2WireError::ActionBodyTooLong {
                maximum: ESP_NOW_V2_MAX_ACTION_LEN,
                actual: input.len(),
            });
        }
        if input[0] != ESP_NOW_ACTION_CATEGORY {
            return Err(EspNowV2WireError::UnsupportedActionCategory(input[0]));
        }
        if input[1..4] != ESP_NOW_ORGANIZATION_IDENTIFIER {
            return Err(EspNowV2WireError::InvalidActionOrganizationIdentifier);
        }

        let elements = &input[ESP_NOW_V2_ACTION_PREFIX_LEN..];
        let mut offset = 0;
        let mut element_count = 0;
        let mut payload_length = 0;
        while offset < elements.len() {
            if element_count == ESP_NOW_V2_MAX_ELEMENT_COUNT {
                return Err(EspNowV2WireError::TooManyElements {
                    maximum: ESP_NOW_V2_MAX_ELEMENT_COUNT,
                });
            }
            let remaining = &elements[offset..];
            if remaining.len() < 2 {
                return Err(EspNowV2WireError::TruncatedElementHeader {
                    element: element_count,
                    remaining: remaining.len(),
                });
            }
            if remaining[0] != ESP_NOW_VENDOR_ELEMENT_ID {
                return Err(EspNowV2WireError::UnsupportedElementId {
                    element: element_count,
                    id: remaining[0],
                });
            }
            let declared = usize::from(remaining[1]);
            if declared < VENDOR_ELEMENT_FIXED_BODY_LEN {
                return Err(EspNowV2WireError::ElementBodyTooShort {
                    element: element_count,
                    declared: remaining[1],
                });
            }
            let encoded_length = 2 + declared;
            if remaining.len() < encoded_length {
                return Err(EspNowV2WireError::ElementLengthMismatch {
                    element: element_count,
                    declared: remaining[1],
                    actual: remaining.len().saturating_sub(2),
                });
            }
            if remaining[2..5] != ESP_NOW_ORGANIZATION_IDENTIFIER {
                return Err(EspNowV2WireError::InvalidElementOrganizationIdentifier {
                    element: element_count,
                });
            }
            if remaining[5] != ESP_NOW_VENDOR_ELEMENT_TYPE {
                return Err(EspNowV2WireError::UnsupportedElementType {
                    element: element_count,
                    element_type: remaining[5],
                });
            }
            let control = remaining[6];
            if control & V2_RESERVED_MASK != 0 {
                return Err(EspNowV2WireError::ReservedControlBitsSet {
                    element: element_count,
                    control,
                });
            }
            let version = control & VERSION_MASK;
            if version != ESP_NOW_V2_VERSION {
                return Err(EspNowV2WireError::UnsupportedVersion {
                    element: element_count,
                    version,
                });
            }

            let next_offset = offset + encoded_length;
            let has_following_element = next_offset < elements.len();
            let more_data = control & V2_MORE_DATA_FLAG != 0;
            if has_following_element && !more_data {
                return Err(EspNowV2WireError::MoreDataClearedBeforeFinal {
                    element: element_count,
                });
            }
            if !has_following_element && more_data {
                return Err(EspNowV2WireError::MoreDataSetOnFinal {
                    element: element_count,
                });
            }

            payload_length += declared - VENDOR_ELEMENT_FIXED_BODY_LEN;
            if payload_length > ESP_NOW_V2_MAX_PAYLOAD_LEN {
                return Err(EspNowV2WireError::PayloadTooLong {
                    length: payload_length,
                });
            }
            element_count += 1;
            offset = next_offset;
        }

        Ok(Self {
            random_value: EspNowRandomValue::new([input[4], input[5], input[6], input[7]]),
            content: EspNowV2ActionContent::EncodedElements(elements),
            payload_length,
            element_count,
            vendor_content_length: elements.len(),
        })
    }
}

const fn element_count_for_payload(payload_length: usize) -> usize {
    if payload_length == 0 {
        1
    } else {
        payload_length.div_ceil(ESP_NOW_V2_MAX_ELEMENT_PAYLOAD_LEN)
    }
}

/// Complete plaintext ESP-NOW v2 MPDU without the hardware-generated FCS.
#[derive(Clone, Copy, Debug)]
pub struct EspNowV2Frame<'payload> {
    destination: EspNowDestination,
    source: EspNowUnicastAddress,
    sequence_number: SequenceNumber,
    retry: bool,
    action: EspNowV2Action<'payload>,
}

impl<'payload> EspNowV2Frame<'payload> {
    pub const fn new(
        destination: EspNowDestination,
        source: EspNowUnicastAddress,
        sequence_number: SequenceNumber,
        random_value: EspNowRandomValue,
        payload: &'payload [u8],
    ) -> Result<Self, EspNowV2WireError> {
        let action = match EspNowV2Action::new(random_value, payload) {
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

    pub const fn action(self) -> EspNowV2Action<'payload> {
        self.action
    }

    pub const fn encoded_len(self) -> usize {
        ESP_NOW_MANAGEMENT_HEADER_LEN + self.action.encoded_len()
    }

    /// Encode an Action MPDU with broadcast BSSID and no FCS.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, EspNowV2WireError> {
        let required = self.encoded_len();
        if output.len() < required {
            return Err(EspNowV2WireError::OutputTooSmall { required });
        }

        let output = &mut output[..required];
        output[..ESP_NOW_MANAGEMENT_HEADER_LEN].fill(0);
        let frame_control = ACTION_FRAME_CONTROL | if self.retry { RETRY_FLAG } else { 0 };
        output[0..2].copy_from_slice(&frame_control.to_le_bytes());
        output[4..10].copy_from_slice(&self.destination.bytes());
        output[10..16].copy_from_slice(&self.source.bytes());
        output[16..22].copy_from_slice(&BROADCAST_ADDRESS);
        output[22..24].copy_from_slice(&self.sequence_number.sequence_control().to_le_bytes());
        self.action
            .encode(&mut output[ESP_NOW_MANAGEMENT_HEADER_LEN..])?;
        Ok(required)
    }

    /// Parse one complete unprotected, MAC-unfragmented v2 Action MPDU.
    pub fn parse(input: &'payload [u8]) -> Result<Self, EspNowV2WireError> {
        if input.len() < ESP_NOW_MANAGEMENT_HEADER_LEN {
            return Err(EspNowV2WireError::FrameTooShort {
                minimum: ESP_NOW_MANAGEMENT_HEADER_LEN,
            });
        }
        let frame_control = u16::from_le_bytes([input[0], input[1]]);
        if frame_control & PROTOCOL_VERSION_MASK != 0
            || frame_control & FRAME_TYPE_AND_SUBTYPE_MASK != ACTION_FRAME_CONTROL
            || frame_control & TO_FROM_DS_MASK != 0
            || frame_control & (POWER_MANAGEMENT_FLAG | MORE_DATA_FLAG | ORDER_FLAG) != 0
        {
            return Err(EspNowV2WireError::UnsupportedFrameControl(frame_control));
        }
        if frame_control & PROTECTED_FLAG != 0 {
            return Err(EspNowV2WireError::ProtectedFrameUnsupported);
        }

        let destination =
            EspNowDestination::new([input[4], input[5], input[6], input[7], input[8], input[9]])
                .map_err(EspNowV2WireError::InvalidDestination)?;
        let source = EspNowUnicastAddress::new([
            input[10], input[11], input[12], input[13], input[14], input[15],
        ])
        .map_err(EspNowV2WireError::InvalidSource)?;
        if input[16..22] != BROADCAST_ADDRESS {
            return Err(EspNowV2WireError::InvalidBssid);
        }
        let sequence_control = u16::from_le_bytes([input[22], input[23]]);
        if frame_control & MORE_FRAGMENTS_FLAG != 0 || sequence_control & 0x000f != 0 {
            return Err(EspNowV2WireError::FragmentedFrame(
                (sequence_control & 0x000f) as u8,
            ));
        }
        let action = EspNowV2Action::parse(&input[ESP_NOW_MANAGEMENT_HEADER_LEN..])?;
        Ok(Self {
            destination,
            source,
            sequence_number: SequenceNumber::from_sequence_control(sequence_control),
            retry: frame_control & RETRY_FLAG != 0,
            action,
        })
    }
}

/// Why bytes cannot cross the strict plaintext ESP-NOW v2 boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowV2WireError {
    PayloadTooLong {
        length: usize,
    },
    OutputTooSmall {
        required: usize,
    },
    ReassemblyCapacityTooSmall {
        required: usize,
        capacity: usize,
    },
    FrameTooShort {
        minimum: usize,
    },
    ActionBodyTooLong {
        maximum: usize,
        actual: usize,
    },
    InvalidDestination(EspNowAddressError),
    InvalidSource(EspNowAddressError),
    InvalidBssid,
    UnsupportedFrameControl(u16),
    ProtectedFrameUnsupported,
    FragmentedFrame(u8),
    UnsupportedActionCategory(u8),
    InvalidActionOrganizationIdentifier,
    TooManyElements {
        maximum: usize,
    },
    TruncatedElementHeader {
        element: usize,
        remaining: usize,
    },
    UnsupportedElementId {
        element: usize,
        id: u8,
    },
    ElementBodyTooShort {
        element: usize,
        declared: u8,
    },
    ElementLengthMismatch {
        element: usize,
        declared: u8,
        actual: usize,
    },
    InvalidElementOrganizationIdentifier {
        element: usize,
    },
    UnsupportedElementType {
        element: usize,
        element_type: u8,
    },
    ReservedControlBitsSet {
        element: usize,
        control: u8,
    },
    UnsupportedVersion {
        element: usize,
        version: u8,
    },
    MoreDataClearedBeforeFinal {
        element: usize,
    },
    MoreDataSetOnFinal {
        element: usize,
    },
}

impl fmt::Display for EspNowV2WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLong { length } => write!(
                formatter,
                "ESP-NOW v2 payload length {length} exceeds {ESP_NOW_V2_MAX_PAYLOAD_LEN} bytes"
            ),
            Self::OutputTooSmall { required } => {
                write!(formatter, "ESP-NOW v2 output needs {required} bytes")
            }
            Self::ReassemblyCapacityTooSmall { required, capacity } => write!(
                formatter,
                "ESP-NOW v2 reassembly needs {required} bytes but capacity is {capacity}"
            ),
            Self::FrameTooShort { minimum } => {
                write!(
                    formatter,
                    "ESP-NOW v2 input is shorter than {minimum} bytes"
                )
            }
            Self::ActionBodyTooLong { maximum, actual } => write!(
                formatter,
                "ESP-NOW v2 Action body has {actual} bytes, exceeding {maximum}"
            ),
            Self::InvalidDestination(error) => write!(formatter, "invalid destination: {error}"),
            Self::InvalidSource(error) => write!(formatter, "invalid source: {error}"),
            Self::InvalidBssid => formatter.write_str("ESP-NOW v2 BSSID is not broadcast"),
            Self::UnsupportedFrameControl(frame_control) => write!(
                formatter,
                "unsupported ESP-NOW v2 frame-control value {frame_control:#06x}"
            ),
            Self::ProtectedFrameUnsupported => {
                formatter.write_str("protected ESP-NOW frames are outside the v2 plaintext profile")
            }
            Self::FragmentedFrame(fragment) => write!(
                formatter,
                "ESP-NOW v2 MAC fragment {fragment} is unsupported"
            ),
            Self::UnsupportedActionCategory(category) => {
                write!(
                    formatter,
                    "unsupported ESP-NOW v2 Action category {category}"
                )
            }
            Self::InvalidActionOrganizationIdentifier => {
                formatter.write_str("invalid ESP-NOW v2 Action organization identifier")
            }
            Self::TooManyElements { maximum } => write!(
                formatter,
                "ESP-NOW v2 Action contains more than {maximum} vendor elements"
            ),
            Self::TruncatedElementHeader { element, remaining } => write!(
                formatter,
                "ESP-NOW v2 element {element} has a {remaining}-byte truncated header"
            ),
            Self::UnsupportedElementId { element, id } => write!(
                formatter,
                "ESP-NOW v2 element {element} has unsupported id {id}"
            ),
            Self::ElementBodyTooShort { element, declared } => write!(
                formatter,
                "ESP-NOW v2 element {element} declares only {declared} body bytes"
            ),
            Self::ElementLengthMismatch {
                element,
                declared,
                actual,
            } => write!(
                formatter,
                "ESP-NOW v2 element {element} declares {declared} bytes but contains {actual}"
            ),
            Self::InvalidElementOrganizationIdentifier { element } => write!(
                formatter,
                "ESP-NOW v2 element {element} has an invalid organization identifier"
            ),
            Self::UnsupportedElementType {
                element,
                element_type,
            } => write!(
                formatter,
                "ESP-NOW v2 element {element} has unsupported type {element_type}"
            ),
            Self::ReservedControlBitsSet { element, control } => write!(
                formatter,
                "ESP-NOW v2 element {element} control {control:#04x} has reserved bits set"
            ),
            Self::UnsupportedVersion { element, version } => write!(
                formatter,
                "ESP-NOW v2 element {element} has unsupported version {version}"
            ),
            Self::MoreDataClearedBeforeFinal { element } => write!(
                formatter,
                "ESP-NOW v2 element {element} clears More data before the final element"
            ),
            Self::MoreDataSetOnFinal { element } => write!(
                formatter,
                "ESP-NOW v2 final element {element} keeps More data set"
            ),
        }
    }
}

impl core::error::Error for EspNowV2WireError {}
