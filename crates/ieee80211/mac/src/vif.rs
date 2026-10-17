//! Fact-only receive routing for one same-channel STA plus SoftAP pair.
//!
//! The classifier interprets only public IEEE 802.11 header fields. It does
//! not infer association, authorization, key ownership or hardware policy;
//! role-local consumers must still validate those semantics after routing.

const FRAME_TYPE_MASK: u16 = 0x000c;
const MANAGEMENT_FRAME: u16 = 0x0000;
const CONTROL_FRAME: u16 = 0x0004;
const DATA_FRAME: u16 = 0x0008;
const TO_DS: u16 = 0x0100;
const FROM_DS: u16 = 0x0200;
const PROBE_REQUEST_SUBTYPE: u16 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaApRxAddresses {
    pub station: [u8; 6],
    pub station_bssid: [u8; 6],
    pub access_point: [u8; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApVif {
    Station,
    AccessPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApRxRoute {
    Interface(StaApVif),
    Foreign,
    Ambiguous,
    Malformed,
}

/// Route one MPDU without assigning meaning beyond its public header.
///
/// A group-addressed frame is routed only when its transmitter/BSSID proves
/// the local interface. Broadcast probe requests are the one deliberate AP
/// exception: a connected station does not own active-scan requests, while a
/// SoftAP must receive them before it can answer.
pub fn classify_sta_ap_rx(mpdu: &[u8], addresses: StaApRxAddresses) -> StaApRxRoute {
    let Some(frame_control) = read_u16(mpdu, 0) else {
        return StaApRxRoute::Malformed;
    };
    let Some(receiver) = address(mpdu, 4) else {
        return StaApRxRoute::Malformed;
    };
    match frame_control & FRAME_TYPE_MASK {
        CONTROL_FRAME => select(
            receiver == addresses.station,
            receiver == addresses.access_point,
        ),
        MANAGEMENT_FRAME => classify_management(mpdu, frame_control, receiver, addresses),
        DATA_FRAME => classify_data(mpdu, frame_control, receiver, addresses),
        _ => StaApRxRoute::Foreign,
    }
}

fn classify_management(
    mpdu: &[u8],
    frame_control: u16,
    receiver: [u8; 6],
    addresses: StaApRxAddresses,
) -> StaApRxRoute {
    let Some(transmitter) = address(mpdu, 10) else {
        return StaApRxRoute::Malformed;
    };
    let Some(bssid) = address(mpdu, 16) else {
        return StaApRxRoute::Malformed;
    };
    let group_receiver = is_group(receiver);
    let subtype = frame_control >> 4 & 0x0f;
    let station = receiver == addresses.station
        || group_receiver
            && (transmitter == addresses.station_bssid || bssid == addresses.station_bssid);
    let access_point = receiver == addresses.access_point
        || bssid == addresses.access_point
        || group_receiver && is_group(bssid) && subtype == PROBE_REQUEST_SUBTYPE;
    select(station, access_point)
}

fn classify_data(
    mpdu: &[u8],
    frame_control: u16,
    receiver: [u8; 6],
    addresses: StaApRxAddresses,
) -> StaApRxRoute {
    let Some(transmitter) = address(mpdu, 10) else {
        return StaApRxRoute::Malformed;
    };
    let Some(bssid_or_destination) = address(mpdu, 16) else {
        return StaApRxRoute::Malformed;
    };
    let group_receiver = is_group(receiver);
    match (frame_control & TO_DS != 0, frame_control & FROM_DS != 0) {
        (false, true) => select(
            receiver == addresses.station
                || group_receiver && transmitter == addresses.station_bssid,
            false,
        ),
        (true, false) => select(false, receiver == addresses.access_point),
        (false, false) => select(
            receiver == addresses.station
                || group_receiver && bssid_or_destination == addresses.station_bssid,
            receiver == addresses.access_point
                || group_receiver && bssid_or_destination == addresses.access_point,
        ),
        (true, true) => StaApRxRoute::Foreign,
    }
}

const fn select(station: bool, access_point: bool) -> StaApRxRoute {
    match (station, access_point) {
        (true, false) => StaApRxRoute::Interface(StaApVif::Station),
        (false, true) => StaApRxRoute::Interface(StaApVif::AccessPoint),
        (false, false) => StaApRxRoute::Foreign,
        (true, true) => StaApRxRoute::Ambiguous,
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
    ]))
}

fn address(bytes: &[u8], offset: usize) -> Option<[u8; 6]> {
    bytes.get(offset..offset + 6)?.try_into().ok()
}

const fn is_group(address: [u8; 6]) -> bool {
    address[0] & 1 != 0
}

#[cfg(test)]
mod tests;
