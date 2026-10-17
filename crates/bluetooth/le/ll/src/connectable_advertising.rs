//! Legacy connectable advertising PDUs: `ADV_IND` with its Channel Selection
//! Algorithm #2 capability and the matching `SCAN_RSP`, and the directed
//! `ADV_DIRECT_IND`.

use crate::{
    LeDeviceAddress, LeDeviceAddressKind,
    advertising::{
        LegacyAdvertisingData, LegacyAdvertisingDataError, LegacyAdvertisingEncodeError,
    },
};

const ADVERTISING_HEADER_LENGTH: usize = 2;
const DEVICE_ADDRESS_LENGTH: usize = 6;
const ADV_IND_TYPE: u8 = 0;
const ADV_DIRECT_IND_TYPE: u8 = 1;
const RX_ADD_RANDOM: u8 = 1 << 7;
const SCAN_RSP_TYPE: u8 = 4;
const PDU_TYPE_MASK: u8 = 0x0f;
const RESERVED_HEADER_BITS: u8 = (1 << 4) | (1 << 7);
const CHANNEL_SELECTION_TWO: u8 = 1 << 5;
const TX_ADD_RANDOM: u8 = 1 << 6;

/// Whether this advertiser may negotiate Channel Selection Algorithm #2.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeChannelSelectionAlgorithmTwoSupport {
    Unsupported,
    Supported,
}

/// Semantic `ADV_IND` payload and channel-selection capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyConnectableAdvertisement<'a> {
    advertiser: LeDeviceAddress,
    data: LegacyAdvertisingData<'a>,
    channel_selection_two: LeChannelSelectionAlgorithmTwoSupport,
}

impl<'a> LegacyConnectableAdvertisement<'a> {
    pub const fn new(
        advertiser: LeDeviceAddress,
        data: LegacyAdvertisingData<'a>,
        channel_selection_two: LeChannelSelectionAlgorithmTwoSupport,
    ) -> Self {
        Self {
            advertiser,
            data,
            channel_selection_two,
        }
    }

    pub const fn advertiser(self) -> LeDeviceAddress {
        self.advertiser
    }

    pub const fn data(self) -> LegacyAdvertisingData<'a> {
        self.data
    }

    pub const fn channel_selection_two(self) -> LeChannelSelectionAlgorithmTwoSupport {
        self.channel_selection_two
    }

    pub const fn encoded_len(self) -> usize {
        ADVERTISING_HEADER_LENGTH + DEVICE_ADDRESS_LENGTH + self.data.len()
    }

    /// Encode the complete Link Layer PDU into bounded caller storage.
    pub fn encode(self, destination: &mut [u8]) -> Result<usize, LegacyAdvertisingEncodeError> {
        let required = self.encoded_len();
        if destination.len() < required {
            return Err(LegacyAdvertisingEncodeError::DestinationTooSmall {
                required,
                available: destination.len(),
            });
        }

        destination[0] = match self.advertiser.kind() {
            LeDeviceAddressKind::Public => 0,
            LeDeviceAddressKind::Random => TX_ADD_RANDOM,
        } | match self.channel_selection_two {
            LeChannelSelectionAlgorithmTwoSupport::Unsupported => 0,
            LeChannelSelectionAlgorithmTwoSupport::Supported => CHANNEL_SELECTION_TWO,
        };
        destination[1] = (DEVICE_ADDRESS_LENGTH + self.data.len()) as u8;
        destination[2..8].copy_from_slice(&self.advertiser.wire_bytes());
        destination[8..required].copy_from_slice(self.data.as_bytes());
        Ok(required)
    }

    /// Decode one exact legacy `ADV_IND` PDU.
    pub fn decode(source: &'a [u8]) -> Result<Self, LegacyConnectableAdvertisementDecodeError> {
        if source.len() < ADVERTISING_HEADER_LENGTH {
            return Err(LegacyConnectableAdvertisementDecodeError::TruncatedHeader {
                available: source.len(),
            });
        }

        let header = source[0];
        let pdu_type = header & PDU_TYPE_MASK;
        if pdu_type != ADV_IND_TYPE {
            return Err(LegacyConnectableAdvertisementDecodeError::UnexpectedPduType { pdu_type });
        }
        if header & RESERVED_HEADER_BITS != 0 {
            return Err(LegacyConnectableAdvertisementDecodeError::ReservedHeaderBitsSet);
        }

        let payload_length = source[1] as usize;
        if !(DEVICE_ADDRESS_LENGTH
            ..=DEVICE_ADDRESS_LENGTH + crate::advertising::LEGACY_ADVERTISING_DATA_CAPACITY)
            .contains(&payload_length)
        {
            return Err(
                LegacyConnectableAdvertisementDecodeError::InvalidPayloadLength {
                    length: payload_length,
                },
            );
        }
        let required = ADVERTISING_HEADER_LENGTH + payload_length;
        if source.len() != required {
            return Err(LegacyConnectableAdvertisementDecodeError::LengthMismatch {
                declared: required,
                available: source.len(),
            });
        }

        let mut address = [0; DEVICE_ADDRESS_LENGTH];
        address.copy_from_slice(&source[2..8]);
        let data = LegacyAdvertisingData::new(&source[8..required])
            .expect("the checked legacy payload bounds its advertising data");
        Ok(Self::new(
            LeDeviceAddress::from_wire_bytes(
                address,
                if header & TX_ADD_RANDOM == 0 {
                    LeDeviceAddressKind::Public
                } else {
                    LeDeviceAddressKind::Random
                },
            ),
            data,
            if header & CHANNEL_SELECTION_TWO == 0 {
                LeChannelSelectionAlgorithmTwoSupport::Unsupported
            } else {
                LeChannelSelectionAlgorithmTwoSupport::Supported
            },
        ))
    }
}

/// Semantic `ADV_DIRECT_IND`: connectable advertising addressed to one
/// initiator, which neither carries data nor answers scan requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyDirectedAdvertisement {
    advertiser: LeDeviceAddress,
    target: LeDeviceAddress,
    channel_selection_two: LeChannelSelectionAlgorithmTwoSupport,
}

impl LegacyDirectedAdvertisement {
    pub const fn new(
        advertiser: LeDeviceAddress,
        target: LeDeviceAddress,
        channel_selection_two: LeChannelSelectionAlgorithmTwoSupport,
    ) -> Self {
        Self {
            advertiser,
            target,
            channel_selection_two,
        }
    }

    pub const fn advertiser(self) -> LeDeviceAddress {
        self.advertiser
    }

    /// The only initiator whose connection indication the set accepts.
    pub const fn target(self) -> LeDeviceAddress {
        self.target
    }

    pub const fn encoded_len(self) -> usize {
        ADVERTISING_HEADER_LENGTH + 2 * DEVICE_ADDRESS_LENGTH
    }

    /// Encode the complete Link Layer PDU into bounded caller storage.
    pub fn encode(self, destination: &mut [u8]) -> Result<usize, LegacyAdvertisingEncodeError> {
        let required = self.encoded_len();
        if destination.len() < required {
            return Err(LegacyAdvertisingEncodeError::DestinationTooSmall {
                required,
                available: destination.len(),
            });
        }
        destination[0] = ADV_DIRECT_IND_TYPE
            | match self.advertiser.kind() {
                LeDeviceAddressKind::Public => 0,
                LeDeviceAddressKind::Random => TX_ADD_RANDOM,
            }
            | match self.target.kind() {
                LeDeviceAddressKind::Public => 0,
                LeDeviceAddressKind::Random => RX_ADD_RANDOM,
            }
            | match self.channel_selection_two {
                LeChannelSelectionAlgorithmTwoSupport::Unsupported => 0,
                LeChannelSelectionAlgorithmTwoSupport::Supported => CHANNEL_SELECTION_TWO,
            };
        destination[1] = (2 * DEVICE_ADDRESS_LENGTH) as u8;
        destination[2..8].copy_from_slice(&self.advertiser.wire_bytes());
        destination[8..14].copy_from_slice(&self.target.wire_bytes());
        Ok(required)
    }
}

/// Semantic Host data carried by the matching legacy `SCAN_RSP`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyScanResponseData<'a>(LegacyAdvertisingData<'a>);

impl<'a> LegacyScanResponseData<'a> {
    /// Validate caller-owned scan-response data without copying it.
    pub const fn new(bytes: &'a [u8]) -> Result<Self, LegacyAdvertisingDataError> {
        match LegacyAdvertisingData::new(bytes) {
            Ok(data) => Ok(Self(data)),
            Err(error) => Err(error),
        }
    }

    /// Borrow the validated response data.
    pub const fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Number of response-data octets.
    pub const fn len(self) -> usize {
        self.0.len()
    }

    /// Whether the response carries no Host data after AdvA.
    pub const fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// Encode the complete `SCAN_RSP` of `advertiser` into bounded caller
    /// storage.
    pub fn encode(
        self,
        advertiser: LeDeviceAddress,
        destination: &mut [u8],
    ) -> Result<usize, LegacyAdvertisingEncodeError> {
        let length = ADVERTISING_HEADER_LENGTH + DEVICE_ADDRESS_LENGTH + self.len();
        if destination.len() < length {
            return Err(LegacyAdvertisingEncodeError::DestinationTooSmall {
                required: length,
                available: destination.len(),
            });
        }
        destination[0] = SCAN_RSP_TYPE
            | match advertiser.kind() {
                LeDeviceAddressKind::Public => 0,
                LeDeviceAddressKind::Random => TX_ADD_RANDOM,
            };
        destination[1] = (DEVICE_ADDRESS_LENGTH + self.len()) as u8;
        destination[2..8].copy_from_slice(&advertiser.wire_bytes());
        destination[8..length].copy_from_slice(self.as_bytes());
        Ok(length)
    }
}

impl LegacyScanResponseData<'static> {
    /// Copy one ephemeral Host response into an async-safe owner.
    pub const fn new_owned(bytes: &[u8]) -> Result<Self, LegacyAdvertisingDataError> {
        match LegacyAdvertisingData::new_owned(bytes) {
            Ok(data) => Ok(Self(data)),
            Err(error) => Err(error),
        }
    }
}

/// Malformed or unsupported `ADV_IND` input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisementDecodeError {
    TruncatedHeader { available: usize },
    UnexpectedPduType { pdu_type: u8 },
    ReservedHeaderBitsSet,
    InvalidPayloadLength { length: usize },
    LengthMismatch { declared: usize, available: usize },
}

#[cfg(test)]
mod tests;
