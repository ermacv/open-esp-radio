//! Legacy connectable advertising PDUs: `ADV_IND` with its Channel Selection
//! Algorithm #2 capability and the matching `SCAN_RSP`, and the directed
//! `ADV_DIRECT_IND`.

use oer_wire::{Reader, U8};

use crate::{
    LE_DEVICE_ADDRESS_LENGTH, LeDeviceAddress, LeDeviceAddressKind,
    advertising::{
        LegacyAdvertisingData, LegacyAdvertisingDataError, LegacyAdvertisingEncodeError,
    },
};

use crate::advertising_header::{
    ADVERTISING_HEADER_LENGTH, AdvertisingHeader, AdvertisingPduType, encode_legacy_pdu,
};

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
        (ADVERTISING_HEADER_LENGTH + LE_DEVICE_ADDRESS_LENGTH).saturating_add(self.data.len())
    }

    /// Encode the complete Link Layer PDU into bounded caller storage.
    pub fn encode(self, destination: &mut [u8]) -> Result<usize, LegacyAdvertisingEncodeError> {
        let header = AdvertisingHeader::ADV_IND
            .with_tx_add_random(matches!(
                self.advertiser.kind(),
                LeDeviceAddressKind::Random
            ))
            .with_channel_selection_two(matches!(
                self.channel_selection_two,
                LeChannelSelectionAlgorithmTwoSupport::Supported
            ))
            .with_length(U8::from_low_bits(
                LE_DEVICE_ADDRESS_LENGTH.saturating_add(self.data.len()) as u8,
            ));
        encode_legacy_pdu(
            header,
            &[&self.advertiser.wire_bytes(), self.data.as_bytes()],
            destination,
        )
    }

    /// Decode one exact legacy `ADV_IND` PDU.
    pub fn decode(source: &'a [u8]) -> Result<Self, LegacyConnectableAdvertisementDecodeError> {
        let mut reader = Reader::new(source);
        let header =
            AdvertisingHeader::from_bytes(reader.read::<ADVERTISING_HEADER_LENGTH>().map_err(
                |_| LegacyConnectableAdvertisementDecodeError::TruncatedHeader {
                    available: source.len(),
                },
            )?);
        if !matches!(header.pdu_type(), AdvertisingPduType::AdvInd) {
            return Err(
                LegacyConnectableAdvertisementDecodeError::UnexpectedPduType {
                    pdu_type: header.pdu_type().encoding(),
                },
            );
        }
        if header.reserved() || header.rx_add_random() {
            return Err(LegacyConnectableAdvertisementDecodeError::ReservedHeaderBitsSet);
        }

        let payload_length = header.length().get() as usize;
        if !(LE_DEVICE_ADDRESS_LENGTH
            ..=LE_DEVICE_ADDRESS_LENGTH + crate::advertising::LEGACY_ADVERTISING_DATA_CAPACITY)
            .contains(&payload_length)
        {
            return Err(
                LegacyConnectableAdvertisementDecodeError::InvalidPayloadLength {
                    length: payload_length,
                },
            );
        }
        let required = ADVERTISING_HEADER_LENGTH.saturating_add(payload_length);
        if source.len() != required {
            return Err(LegacyConnectableAdvertisementDecodeError::LengthMismatch {
                declared: required,
                available: source.len(),
            });
        }

        let address = reader.read::<LE_DEVICE_ADDRESS_LENGTH>().map_err(|_| {
            LegacyConnectableAdvertisementDecodeError::InvalidPayloadLength {
                length: payload_length,
            }
        })?;
        let data = reader.remaining();
        let data = LegacyAdvertisingData::new(data).map_err(|_| {
            LegacyConnectableAdvertisementDecodeError::InvalidPayloadLength {
                length: payload_length,
            }
        })?;
        Ok(Self::new(
            header.advertiser(address),
            data,
            if !header.channel_selection_two() {
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
        ADVERTISING_HEADER_LENGTH + 2 * LE_DEVICE_ADDRESS_LENGTH
    }

    /// Encode the complete Link Layer PDU into bounded caller storage.
    pub fn encode(self, destination: &mut [u8]) -> Result<usize, LegacyAdvertisingEncodeError> {
        let header = AdvertisingHeader::ADV_DIRECT_IND
            .with_tx_add_random(matches!(
                self.advertiser.kind(),
                LeDeviceAddressKind::Random
            ))
            .with_rx_add_random(matches!(self.target.kind(), LeDeviceAddressKind::Random))
            .with_channel_selection_two(matches!(
                self.channel_selection_two,
                LeChannelSelectionAlgorithmTwoSupport::Supported
            ))
            .with_length(U8::constant::<{ (2 * LE_DEVICE_ADDRESS_LENGTH) as u8 }>());
        encode_legacy_pdu(
            header,
            &[&self.advertiser.wire_bytes(), &self.target.wire_bytes()],
            destination,
        )
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
    pub fn as_bytes(&self) -> &[u8] {
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
        let header = AdvertisingHeader::SCAN_RSP
            .with_tx_add_random(matches!(advertiser.kind(), LeDeviceAddressKind::Random))
            .with_length(U8::from_low_bits(
                LE_DEVICE_ADDRESS_LENGTH.saturating_add(self.len()) as u8,
            ));
        encode_legacy_pdu(
            header,
            &[&advertiser.wire_bytes(), self.as_bytes()],
            destination,
        )
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
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
mod tests;
