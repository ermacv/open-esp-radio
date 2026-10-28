//! Legacy advertising report parsing and duplicate filtering.
//!
//! This module understands Bluetooth air-interface fields only. It contains no
//! ESP32 descriptor layout, MMIO, HCI packet framing, executor, or allocator.

use oer_wire::{PrefixLength, Reader};

use crate::advertising_header::{ADVERTISING_HEADER_LENGTH, AdvertisingHeader, AdvertisingPduType};
use crate::{LE_DEVICE_ADDRESS_LENGTH, LeDeviceAddress, LeDeviceAddressKind};

/// Largest legacy advertising data after the advertiser address.
pub const LEGACY_SCAN_DATA_CAPACITY: usize = 31;

const MAX_LEGACY_PAYLOAD_BYTES: usize = LE_DEVICE_ADDRESS_LENGTH + LEGACY_SCAN_DATA_CAPACITY;

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
    let mut reader = Reader::new(pdu);
    let header = match reader.read::<ADVERTISING_HEADER_LENGTH>() {
        Ok(bytes) => AdvertisingHeader::from_bytes(bytes),
        Err(_) => return Err(LegacyAdvertisingReportParseError::HeaderTruncated),
    };
    let payload_len = header.legacy_length() as usize;
    if payload_len > MAX_LEGACY_PAYLOAD_BYTES {
        return Err(LegacyAdvertisingReportParseError::LegacyPayloadTooLong);
    }
    if pdu.len() != ADVERTISING_HEADER_LENGTH.saturating_add(payload_len) {
        return Err(LegacyAdvertisingReportParseError::DeclaredLengthMismatch);
    }
    if payload_len < LE_DEVICE_ADDRESS_LENGTH {
        return Err(LegacyAdvertisingReportParseError::AdvertiserAddressMissing);
    }

    let kind = match header.pdu_type() {
        AdvertisingPduType::AdvInd => LegacyAdvertisingReportKind::ConnectableUndirected,
        AdvertisingPduType::AdvDirectInd => LegacyAdvertisingReportKind::ConnectableDirected,
        AdvertisingPduType::AdvNonconnInd => LegacyAdvertisingReportKind::NonconnectableUndirected,
        AdvertisingPduType::ScanRsp => LegacyAdvertisingReportKind::ScanResponse,
        AdvertisingPduType::AdvScanInd => LegacyAdvertisingReportKind::ScannableUndirected,
        unsupported => {
            return Err(LegacyAdvertisingReportParseError::UnsupportedPduType(
                unsupported.encoding(),
            ));
        }
    };
    if matches!(kind, LegacyAdvertisingReportKind::ConnectableDirected)
        && payload_len != 2 * LE_DEVICE_ADDRESS_LENGTH
    {
        return Err(LegacyAdvertisingReportParseError::DirectedAddressLengthInvalid);
    }

    let address = match reader.read::<LE_DEVICE_ADDRESS_LENGTH>() {
        Ok(bytes) => bytes,
        Err(_) => return Err(LegacyAdvertisingReportParseError::AdvertiserAddressMissing),
    };
    let advertiser = header.advertiser(address);
    let remaining = reader.remaining();
    let directed_target = if matches!(kind, LegacyAdvertisingReportKind::ConnectableDirected) {
        let Some(address) = remaining.first_chunk::<LE_DEVICE_ADDRESS_LENGTH>() else {
            return Err(LegacyAdvertisingReportParseError::DirectedAddressLengthInvalid);
        };
        Some(LeDeviceAddress::from_wire_bytes(
            *address,
            if !header.rx_add_random() {
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
