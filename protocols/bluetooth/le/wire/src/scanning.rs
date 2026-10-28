//! Legacy advertising report parsing and duplicate filtering.
//!
//! This module understands Bluetooth air-interface fields only. It contains no
//! ESP32 descriptor layout, MMIO, HCI packet framing, executor, or allocator.

use oer_wire::PrefixLength;

use crate::{LeDeviceAddress, LeDeviceAddressKind};

/// Largest legacy advertising data after the advertiser address.
pub const LEGACY_SCAN_DATA_CAPACITY: usize = 31;

const LEGACY_HEADER_BYTES: usize = 2;
const DEVICE_ADDRESS_BYTES: usize = 6;
const MAX_LEGACY_PAYLOAD_BYTES: usize = DEVICE_ADDRESS_BYTES + LEGACY_SCAN_DATA_CAPACITY;
const PDU_TYPE_MASK: u8 = 0x0f;
const TX_ADD_RANDOM: u8 = 1 << 6;
const RX_ADD_RANDOM: u8 = 1 << 7;
const PAYLOAD_LENGTH_MASK: u8 = 0x3f;

/// Primary advertising channel a report was received on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimaryScanChannel {
    Channel37,
    Channel38,
    Channel39,
}

/// Legacy advertising PDU classes reportable by a passive scanner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingReportKind {
    ConnectableUndirected,
    ConnectableDirected,
    NonconnectableUndirected,
    ScanResponse,
    ScannableUndirected,
}

/// Owned protocol report formed from one validated on-air PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingReport {
    kind: LegacyAdvertisingReportKind,
    advertiser: LeDeviceAddress,
    directed_target: Option<LeDeviceAddress>,
    data: [u8; LEGACY_SCAN_DATA_CAPACITY],
    data_len: PrefixLength<LEGACY_SCAN_DATA_CAPACITY>,
    channel: PrimaryScanChannel,
    rssi_dbm: i8,
}

impl LegacyAdvertisingReport {
    pub const fn kind(&self) -> LegacyAdvertisingReportKind {
        self.kind
    }

    pub const fn advertiser(&self) -> LeDeviceAddress {
        self.advertiser
    }

    pub const fn directed_target(&self) -> Option<LeDeviceAddress> {
        self.directed_target
    }

    pub fn data(&self) -> &[u8] {
        self.data_len.borrow(&self.data)
    }

    pub const fn channel(&self) -> PrimaryScanChannel {
        self.channel
    }

    pub const fn rssi_dbm(&self) -> i8 {
        self.rssi_dbm
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingReportParseError {
    HeaderTruncated,
    DeclaredLengthMismatch,
    LegacyPayloadTooLong,
    UnsupportedPduType(u8),
    AdvertiserAddressMissing,
    DirectedAddressLengthInvalid,
}

/// Parse one complete legacy advertising-channel PDU plus receive metadata.
pub const fn parse_legacy_advertising_report(
    pdu: &[u8],
    channel: PrimaryScanChannel,
    rssi_dbm: i8,
) -> Result<LegacyAdvertisingReport, LegacyAdvertisingReportParseError> {
    let Some((header_bytes, payload)) = pdu.split_first_chunk::<LEGACY_HEADER_BYTES>() else {
        return Err(LegacyAdvertisingReportParseError::HeaderTruncated);
    };
    let [header, length] = *header_bytes;
    let payload_len = (length & PAYLOAD_LENGTH_MASK) as usize;
    if payload_len > MAX_LEGACY_PAYLOAD_BYTES {
        return Err(LegacyAdvertisingReportParseError::LegacyPayloadTooLong);
    }
    if pdu.len() != LEGACY_HEADER_BYTES.saturating_add(payload_len) {
        return Err(LegacyAdvertisingReportParseError::DeclaredLengthMismatch);
    }
    if payload_len < DEVICE_ADDRESS_BYTES {
        return Err(LegacyAdvertisingReportParseError::AdvertiserAddressMissing);
    }

    let kind = match header & PDU_TYPE_MASK {
        0 => LegacyAdvertisingReportKind::ConnectableUndirected,
        1 => LegacyAdvertisingReportKind::ConnectableDirected,
        2 => LegacyAdvertisingReportKind::NonconnectableUndirected,
        4 => LegacyAdvertisingReportKind::ScanResponse,
        6 => LegacyAdvertisingReportKind::ScannableUndirected,
        unsupported => {
            return Err(LegacyAdvertisingReportParseError::UnsupportedPduType(
                unsupported,
            ));
        }
    };
    if matches!(kind, LegacyAdvertisingReportKind::ConnectableDirected) && payload_len != 12 {
        return Err(LegacyAdvertisingReportParseError::DirectedAddressLengthInvalid);
    }

    let Some((address, remaining)) = payload.split_first_chunk::<DEVICE_ADDRESS_BYTES>() else {
        return Err(LegacyAdvertisingReportParseError::AdvertiserAddressMissing);
    };
    let advertiser = LeDeviceAddress::from_wire_bytes(
        *address,
        if header & TX_ADD_RANDOM == 0 {
            LeDeviceAddressKind::Public
        } else {
            LeDeviceAddressKind::Random
        },
    );
    let directed_target = if matches!(kind, LegacyAdvertisingReportKind::ConnectableDirected) {
        let Some(address) = remaining.first_chunk::<DEVICE_ADDRESS_BYTES>() else {
            return Err(LegacyAdvertisingReportParseError::DirectedAddressLengthInvalid);
        };
        Some(LeDeviceAddress::from_wire_bytes(
            *address,
            if header & RX_ADD_RANDOM == 0 {
                LeDeviceAddressKind::Public
            } else {
                LeDeviceAddressKind::Random
            },
        ))
    } else {
        None
    };
    let source = if directed_target.is_some() {
        &[]
    } else {
        remaining
    };
    let Some(data_len) = PrefixLength::new(source.len()) else {
        return Err(LegacyAdvertisingReportParseError::LegacyPayloadTooLong);
    };
    let mut data = [0; LEGACY_SCAN_DATA_CAPACITY];
    let mut source = source;
    let mut destination = data.as_mut_slice();
    while let Some((byte, rest)) = source.split_first() {
        let Some((slot, tail)) = destination.split_first_mut() else {
            return Err(LegacyAdvertisingReportParseError::LegacyPayloadTooLong);
        };
        *slot = *byte;
        source = rest;
        destination = tail;
    }
    Ok(LegacyAdvertisingReport {
        kind,
        advertiser,
        directed_target,
        data,
        data_len,
        channel,
        rssi_dbm,
    })
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
mod tests;
