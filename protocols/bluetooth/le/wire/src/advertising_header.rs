//! Advertising physical-channel header (Bluetooth Core 6.0, Vol 6, Part B, 2.3).

use oer_wire::{Reader, Writer};

use crate::{LE_DEVICE_ADDRESS_LENGTH, LeDeviceAddress, LeDeviceAddressKind};

/// Two-octet advertising physical-channel PDU header (Core Vol 6, Part B, 2.3).
pub const ADVERTISING_HEADER_LENGTH: usize = core::mem::size_of::<AdvertisingHeader>();

oer_wire::bitfield! {
    /// Primary advertising-channel PDU types (Core Vol 6, Part B, 2.3, Table 2.3).
    pub enum AdvertisingPduType: u8 {
        /// Connectable undirected advertising, 2.3.1.1.
        AdvInd = 0,
        /// Connectable directed advertising, 2.3.1.2.
        AdvDirectInd = 1,
        /// Non-connectable undirected advertising, 2.3.1.3.
        AdvNonconnInd = 2,
        /// Scan request, 2.3.2.1.
        ScanReq = 3,
        /// Scan response, 2.3.2.2.
        ScanRsp = 4,
        /// Connection request, 2.3.3.1.
        ConnectInd = 5,
        /// Scannable undirected advertising, 2.3.1.4.
        AdvScanInd = 6,
        /// Extended advertising, 2.3.1.5.
        AdvExtInd = 7,
    }
}

oer_wire::bitfield! {
    /// SOURCE: Bluetooth Core 6.0, Vol 6, Part B, 2.3, Figure 2.5.
    pub struct AdvertisingHeader[2] (little) {
        /// PDU Type, Core Vol 6, Part B, 2.3, Table 2.3.
        pub pdu_type, with_pdu_type: [AdvertisingPduType] = 0..4;
        /// RFU, Core Vol 6, Part B, 2.3, Figure 2.5.
        pub reserved, with_reserved: bool = 4..5;
        /// ChSel; reserved unless the PDU uses it, Core Vol 6, Part B, 2.3/2.3.1.
        pub channel_selection_two, with_channel_selection_two: bool = 5..6;
        /// TxAdd (AdvA class for ADV_NONCONN_IND), Core Vol 6, Part B, 2.3/2.3.1.3.
        pub tx_add_random, with_tx_add_random: bool = 6..7;
        /// RxAdd; reserved for ADV_NONCONN_IND, Core Vol 6, Part B, 2.3/2.3.1.3.
        pub rx_add_random, with_rx_add_random: bool = 7..8;
        /// Payload length in octets, Core Vol 6, Part B, 2.3.
        pub length, with_length: (u8) = 8..16;
        /// Legacy six-bit length view (Core 4.2, Vol 6, Part B, 2.3).
        /// Retained for receivers whose admission contract uses that view.
        pub legacy_length, with_legacy_length: u8 = 8..14, view;
        /// Legacy length RFU view (Core 4.2, Vol 6, Part B, 2.3).
        pub legacy_length_reserved, with_legacy_length_reserved: u8 = 14..16, view;
    }
}

impl AdvertisingHeader {
    pub const ADV_IND: Self = match Self::from_bytes([0; ADVERTISING_HEADER_LENGTH])
        .with_pdu_type(AdvertisingPduType::AdvInd)
    {
        Ok(header) => header,
        Err(_) => panic!("defined PDU type fits"),
    };
    pub const ADV_DIRECT_IND: Self = match Self::from_bytes([0; ADVERTISING_HEADER_LENGTH])
        .with_pdu_type(AdvertisingPduType::AdvDirectInd)
    {
        Ok(header) => header,
        Err(_) => panic!("defined PDU type fits"),
    };
    pub const ADV_NONCONN_IND: Self = match Self::from_bytes([0; ADVERTISING_HEADER_LENGTH])
        .with_pdu_type(AdvertisingPduType::AdvNonconnInd)
    {
        Ok(header) => header,
        Err(_) => panic!("defined PDU type fits"),
    };
    pub const ADV_SCAN_IND: Self = match Self::from_bytes([0; ADVERTISING_HEADER_LENGTH])
        .with_pdu_type(AdvertisingPduType::AdvScanInd)
    {
        Ok(header) => header,
        Err(_) => panic!("defined PDU type fits"),
    };
    pub const SCAN_RSP: Self = match Self::from_bytes([0; ADVERTISING_HEADER_LENGTH])
        .with_pdu_type(AdvertisingPduType::ScanRsp)
    {
        Ok(header) => header,
        Err(_) => panic!("defined PDU type fits"),
    };

    pub(crate) const fn advertiser(self, bytes: [u8; LE_DEVICE_ADDRESS_LENGTH]) -> LeDeviceAddress {
        LeDeviceAddress::from_wire_bytes(
            bytes,
            if self.tx_add_random() {
                LeDeviceAddressKind::Random
            } else {
                LeDeviceAddressKind::Public
            },
        )
    }
}

/// Decoded header and leading advertiser address, without payload admission.
///
/// This prefix is for legacy advertisements whose payload begins with AdvA
/// (Core Vol 6, Part B, 2.3.1). It does not validate PDU type, flags or declared
/// length: the consumer selects its supported profile before admitting the
/// complete packet. SCAN_REQ and CONNECT_IND begin with ScanA/InitA instead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingPduPrefix {
    pub header: AdvertisingHeader,
    pub advertiser: LeDeviceAddress,
}

impl LegacyAdvertisingPduPrefix {
    /// Read the complete prefix with the bounded wire cursor.
    pub const fn decode(source: &[u8]) -> Result<Self, oer_wire::Error> {
        let mut reader = Reader::new(source);
        let header = match reader.read::<ADVERTISING_HEADER_LENGTH>() {
            Ok(bytes) => AdvertisingHeader::from_bytes(bytes),
            Err(error) => return Err(error),
        };
        let address = match reader.read::<LE_DEVICE_ADDRESS_LENGTH>() {
            Ok(bytes) => bytes,
            Err(error) => return Err(error),
        };
        Ok(Self {
            header,
            advertiser: header.advertiser(address),
        })
    }
}

pub(crate) fn encode_legacy_pdu(
    header: AdvertisingHeader,
    payload: &[&[u8]],
    destination: &mut [u8],
) -> Result<usize, crate::advertising::LegacyAdvertisingEncodeError> {
    let required = ADVERTISING_HEADER_LENGTH.saturating_add(header.length().get() as usize);
    let available = destination.len();
    let error = || crate::advertising::LegacyAdvertisingEncodeError::DestinationTooSmall {
        required,
        available,
    };
    if available < required {
        return Err(error());
    }
    let mut writer = Writer::new(destination);
    writer.write(&header.to_bytes()).map_err(|_| error())?;
    for field in payload {
        writer.write(field).map_err(|_| error())?;
    }
    Ok(writer.written())
}

#[cfg(test)]
#[allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects
)]
mod tests;
