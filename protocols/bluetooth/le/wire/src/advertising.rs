//! Legacy advertising data and the non-connectable `ADV_NONCONN_IND` and
//! `ADV_SCAN_IND` PDUs.

use oer_wire::{PrefixLength, Reader, U8};

use crate::advertising_header::{
    ADVERTISING_HEADER_LENGTH, AdvertisingHeader, AdvertisingPduType, encode_legacy_pdu,
};
use crate::{LE_DEVICE_ADDRESS_LENGTH, LeDeviceAddress, LeDeviceAddressKind};

/// Maximum Host advertising data carried by a legacy advertising PDU.
pub const LEGACY_ADVERTISING_DATA_CAPACITY: usize = 31;
/// Complete encoded capacity of a legacy advertising PDU header, AdvA and data.
pub const LEGACY_ADVERTISING_PDU_CAPACITY: usize =
    ADVERTISING_HEADER_LENGTH + LE_DEVICE_ADDRESS_LENGTH + LEGACY_ADVERTISING_DATA_CAPACITY;

/// Borrowed or internally owned legacy advertising data with a checked limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingData<'a> {
    storage: LegacyAdvertisingDataStorage<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyAdvertisingDataStorage<'a> {
    Borrowed(&'a [u8]),
    Owned {
        bytes: [u8; LEGACY_ADVERTISING_DATA_CAPACITY],
        length: PrefixLength<LEGACY_ADVERTISING_DATA_CAPACITY>,
    },
}

impl<'a> LegacyAdvertisingData<'a> {
    /// Validate one caller-owned advertising-data value without copying it.
    pub const fn new(bytes: &'a [u8]) -> Result<Self, LegacyAdvertisingDataError> {
        if bytes.len() > LEGACY_ADVERTISING_DATA_CAPACITY {
            return Err(LegacyAdvertisingDataError::TooLong {
                length: bytes.len(),
            });
        }
        Ok(Self {
            storage: LegacyAdvertisingDataStorage::Borrowed(bytes),
        })
    }

    /// Borrow the validated bytes.
    pub fn as_bytes(&self) -> &[u8] {
        match &self.storage {
            LegacyAdvertisingDataStorage::Borrowed(bytes) => bytes,
            LegacyAdvertisingDataStorage::Owned { bytes, length } => length.borrow(bytes),
        }
    }

    /// Number of advertising-data octets.
    pub const fn len(self) -> usize {
        match self.storage {
            LegacyAdvertisingDataStorage::Borrowed(bytes) => bytes.len(),
            LegacyAdvertisingDataStorage::Owned { length, .. } => length.get(),
        }
    }

    /// Whether the advertising-data field is empty.
    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }
}

impl LegacyAdvertisingData<'static> {
    /// Copy one ephemeral Host value into a self-contained async-safe owner.
    pub const fn new_owned(bytes: &[u8]) -> Result<Self, LegacyAdvertisingDataError> {
        let Some(length) = PrefixLength::new(bytes.len()) else {
            return Err(LegacyAdvertisingDataError::TooLong {
                length: bytes.len(),
            });
        };
        let mut owned = [0; LEGACY_ADVERTISING_DATA_CAPACITY];
        let mut source = bytes;
        let mut destination = owned.as_mut_slice();
        while let Some((byte, rest)) = source.split_first() {
            let Some((slot, tail)) = destination.split_first_mut() else {
                return Err(LegacyAdvertisingDataError::TooLong {
                    length: bytes.len(),
                });
            };
            *slot = *byte;
            source = rest;
            destination = tail;
        }
        Ok(Self {
            storage: LegacyAdvertisingDataStorage::Owned {
                bytes: owned,
                length,
            },
        })
    }
}

/// Invalid legacy advertising data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingDataError {
    /// More than 31 octets were supplied.
    TooLong { length: usize },
}

/// Whether a non-connectable advertisement answers scan requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyNonconnectableKind {
    /// `ADV_NONCONN_IND`: neither connectable nor scannable.
    NonScannable,
    /// `ADV_SCAN_IND`: scannable, answered by a `SCAN_RSP`.
    Scannable,
}

impl LegacyNonconnectableKind {
    const fn header(self) -> AdvertisingHeader {
        match self {
            Self::NonScannable => AdvertisingHeader::ADV_NONCONN_IND,
            Self::Scannable => AdvertisingHeader::ADV_SCAN_IND,
        }
    }
}

/// Semantic non-connectable undirected advertisement: `ADV_NONCONN_IND` or
/// `ADV_SCAN_IND`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyNonconnectableAdvertisement<'a> {
    kind: LegacyNonconnectableKind,
    advertiser: LeDeviceAddress,
    data: LegacyAdvertisingData<'a>,
}

impl<'a> LegacyNonconnectableAdvertisement<'a> {
    /// Construct a non-connectable undirected advertisement of `kind`.
    pub const fn new(
        kind: LegacyNonconnectableKind,
        advertiser: LeDeviceAddress,
        data: LegacyAdvertisingData<'a>,
    ) -> Self {
        Self {
            kind,
            advertiser,
            data,
        }
    }

    /// Whether the advertisement answers scan requests.
    pub const fn kind(self) -> LegacyNonconnectableKind {
        self.kind
    }

    /// Advertiser address and TxAdd class.
    pub const fn advertiser(self) -> LeDeviceAddress {
        self.advertiser
    }

    /// Validated Host advertising data.
    pub const fn data(self) -> LegacyAdvertisingData<'a> {
        self.data
    }

    /// Required encoded PDU length.
    pub const fn encoded_len(self) -> usize {
        (ADVERTISING_HEADER_LENGTH + LE_DEVICE_ADDRESS_LENGTH).saturating_add(self.data.len())
    }

    /// Encode the complete Link Layer PDU into caller-owned bounded storage.
    ///
    /// Preamble, advertising Access Address, CRC and whitening are deliberately
    /// outside this codec: the chip backend must state which of those operations
    /// are performed by hardware.
    pub fn encode(self, destination: &mut [u8]) -> Result<usize, LegacyAdvertisingEncodeError> {
        let header = self
            .kind
            .header()
            .with_tx_add_random(matches!(
                self.advertiser.kind(),
                LeDeviceAddressKind::Random
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

    /// Decode one exact `ADV_NONCONN_IND` or `ADV_SCAN_IND` PDU.
    pub fn decode(source: &'a [u8]) -> Result<Self, LegacyAdvertisingDecodeError> {
        let mut reader = Reader::new(source);
        let header =
            AdvertisingHeader::from_bytes(reader.read::<ADVERTISING_HEADER_LENGTH>().map_err(
                |_| LegacyAdvertisingDecodeError::TruncatedHeader {
                    available: source.len(),
                },
            )?);
        let kind = match header.pdu_type() {
            AdvertisingPduType::AdvNonconnInd => LegacyNonconnectableKind::NonScannable,
            AdvertisingPduType::AdvScanInd => LegacyNonconnectableKind::Scannable,
            other => {
                return Err(LegacyAdvertisingDecodeError::UnexpectedPduType {
                    pdu_type: other.encoding(),
                });
            }
        };
        if header.reserved() || header.channel_selection_two() || header.rx_add_random() {
            return Err(LegacyAdvertisingDecodeError::ReservedHeaderBitsSet);
        }

        let payload_length = header.length().get() as usize;
        if !(LE_DEVICE_ADDRESS_LENGTH..=LE_DEVICE_ADDRESS_LENGTH + LEGACY_ADVERTISING_DATA_CAPACITY)
            .contains(&payload_length)
        {
            return Err(LegacyAdvertisingDecodeError::InvalidPayloadLength {
                length: payload_length,
            });
        }
        let required = ADVERTISING_HEADER_LENGTH.saturating_add(payload_length);
        if source.len() != required {
            return Err(LegacyAdvertisingDecodeError::LengthMismatch {
                declared: required,
                available: source.len(),
            });
        }

        let wire_bytes = reader.read::<LE_DEVICE_ADDRESS_LENGTH>().map_err(|_| {
            LegacyAdvertisingDecodeError::InvalidPayloadLength {
                length: payload_length,
            }
        })?;
        let data = reader.remaining();
        let data = LegacyAdvertisingData::new(data).map_err(|_| {
            LegacyAdvertisingDecodeError::InvalidPayloadLength {
                length: payload_length,
            }
        })?;
        Ok(Self::new(kind, header.advertiser(wire_bytes), data))
    }
}

/// Failed bounded PDU encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingEncodeError {
    /// Caller storage cannot retain the complete PDU.
    DestinationTooSmall { required: usize, available: usize },
}

/// Malformed or unsupported advertising PDU input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingDecodeError {
    /// The two-octet advertising header is incomplete.
    TruncatedHeader { available: usize },
    /// The PDU is neither `ADV_NONCONN_IND` nor `ADV_SCAN_IND`.
    UnexpectedPduType { pdu_type: u8 },
    /// ChSel, RxAdd or another reserved header bit was nonzero.
    ReservedHeaderBitsSet,
    /// The payload cannot contain exactly AdvA plus at most 31 data octets.
    InvalidPayloadLength { length: usize },
    /// The input does not end at the declared payload boundary.
    LengthMismatch { declared: usize, available: usize },
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
mod tests;
