//! Legacy advertising data and the non-connectable `ADV_NONCONN_IND` and
//! `ADV_SCAN_IND` PDUs.

use crate::{LeDeviceAddress, LeDeviceAddressKind};

/// Maximum Host advertising data carried by a legacy advertising PDU.
pub const LEGACY_ADVERTISING_DATA_CAPACITY: usize = 31;
/// Complete encoded capacity of a legacy advertising PDU header, AdvA and data.
pub const LEGACY_ADVERTISING_PDU_CAPACITY: usize = 39;

const ADVERTISING_HEADER_LENGTH: usize = 2;
const DEVICE_ADDRESS_LENGTH: usize = 6;
const ADV_NONCONN_IND_TYPE: u8 = 0b0010;
const ADV_SCAN_IND_TYPE: u8 = 0b0110;
const TX_ADD_RANDOM: u8 = 1 << 6;
const NONCONNECTABLE_RESERVED_HEADER_BITS: u8 = (1 << 4) | (1 << 5) | (1 << 7);

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
        length: u8,
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
    pub const fn as_bytes(&self) -> &[u8] {
        match &self.storage {
            LegacyAdvertisingDataStorage::Borrowed(bytes) => bytes,
            LegacyAdvertisingDataStorage::Owned { bytes, length } => {
                bytes.split_at(*length as usize).0
            }
        }
    }

    /// Number of advertising-data octets.
    pub const fn len(self) -> usize {
        match self.storage {
            LegacyAdvertisingDataStorage::Borrowed(bytes) => bytes.len(),
            LegacyAdvertisingDataStorage::Owned { length, .. } => length as usize,
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
        if bytes.len() > LEGACY_ADVERTISING_DATA_CAPACITY {
            return Err(LegacyAdvertisingDataError::TooLong {
                length: bytes.len(),
            });
        }
        let mut owned = [0; LEGACY_ADVERTISING_DATA_CAPACITY];
        let mut index = 0;
        while index < bytes.len() {
            owned[index] = bytes[index];
            index += 1;
        }
        Ok(Self {
            storage: LegacyAdvertisingDataStorage::Owned {
                bytes: owned,
                length: bytes.len() as u8,
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
    const fn pdu_type(self) -> u8 {
        match self {
            Self::NonScannable => ADV_NONCONN_IND_TYPE,
            Self::Scannable => ADV_SCAN_IND_TYPE,
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
        ADVERTISING_HEADER_LENGTH + DEVICE_ADDRESS_LENGTH + self.data.len()
    }

    /// Encode the complete Link Layer PDU into caller-owned bounded storage.
    ///
    /// Preamble, advertising Access Address, CRC and whitening are deliberately
    /// outside this codec: the chip backend must state which of those operations
    /// are performed by hardware.
    pub fn encode(self, destination: &mut [u8]) -> Result<usize, LegacyAdvertisingEncodeError> {
        let required = self.encoded_len();
        if destination.len() < required {
            return Err(LegacyAdvertisingEncodeError::DestinationTooSmall {
                required,
                available: destination.len(),
            });
        }

        let tx_add = match self.advertiser.kind() {
            LeDeviceAddressKind::Public => 0,
            LeDeviceAddressKind::Random => TX_ADD_RANDOM,
        };
        destination[0] = self.kind.pdu_type() | tx_add;
        destination[1] = (DEVICE_ADDRESS_LENGTH + self.data.len()) as u8;
        destination[2..8].copy_from_slice(&self.advertiser.wire_bytes());
        destination[8..required].copy_from_slice(self.data.as_bytes());
        Ok(required)
    }

    /// Decode one exact `ADV_NONCONN_IND` or `ADV_SCAN_IND` PDU.
    pub fn decode(source: &'a [u8]) -> Result<Self, LegacyAdvertisingDecodeError> {
        if source.len() < ADVERTISING_HEADER_LENGTH {
            return Err(LegacyAdvertisingDecodeError::TruncatedHeader {
                available: source.len(),
            });
        }

        let header = source[0];
        let kind = match header & 0x0f {
            ADV_NONCONN_IND_TYPE => LegacyNonconnectableKind::NonScannable,
            ADV_SCAN_IND_TYPE => LegacyNonconnectableKind::Scannable,
            pdu_type => {
                return Err(LegacyAdvertisingDecodeError::UnexpectedPduType { pdu_type });
            }
        };
        if header & NONCONNECTABLE_RESERVED_HEADER_BITS != 0 {
            return Err(LegacyAdvertisingDecodeError::ReservedHeaderBitsSet);
        }

        let payload_length = source[1] as usize;
        if !(DEVICE_ADDRESS_LENGTH..=DEVICE_ADDRESS_LENGTH + LEGACY_ADVERTISING_DATA_CAPACITY)
            .contains(&payload_length)
        {
            return Err(LegacyAdvertisingDecodeError::InvalidPayloadLength {
                length: payload_length,
            });
        }
        let required = ADVERTISING_HEADER_LENGTH + payload_length;
        if source.len() != required {
            return Err(LegacyAdvertisingDecodeError::LengthMismatch {
                declared: required,
                available: source.len(),
            });
        }

        let mut wire_bytes = [0; DEVICE_ADDRESS_LENGTH];
        wire_bytes.copy_from_slice(&source[2..8]);
        let address_kind = if header & TX_ADD_RANDOM == 0 {
            LeDeviceAddressKind::Public
        } else {
            LeDeviceAddressKind::Random
        };
        let data = LegacyAdvertisingData::new(&source[8..required])
            .expect("the checked legacy payload length bounds its advertising data");
        Ok(Self::new(
            kind,
            LeDeviceAddress::from_wire_bytes(wire_bytes, address_kind),
            data,
        ))
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
mod tests;
