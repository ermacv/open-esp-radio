//! Receive routing for one same-channel STA plus SoftAP pair.
//!
//! The classifier interprets only public IEEE 802.11 header fields. It does
//! not infer association, authorization, key ownership or hardware policy;
//! role-local consumers must still validate those semantics after routing.
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use oer_ieee80211_wire::{
    address::AddressFlags,
    frame::{AddressFields, FrameType, ManagementSubtype},
};

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
    let Ok(fields) = AddressFields::parse(mpdu) else {
        return StaApRxRoute::Malformed;
    };
    let receiver = fields.receiver();
    match fields.control().kind() {
        FrameType::Control => select(
            receiver == addresses.station,
            receiver == addresses.access_point,
        ),
        FrameType::Management => classify_management(fields, addresses),
        FrameType::Data => classify_data(fields, addresses),
        _ => StaApRxRoute::Foreign,
    }
}

fn classify_management(fields: AddressFields, addresses: StaApRxAddresses) -> StaApRxRoute {
    let Some((transmitter, bssid)) = fields.transmitter().zip(fields.address3()) else {
        return StaApRxRoute::Malformed;
    };
    let receiver = fields.receiver();
    let group_receiver = is_group(receiver);
    let probe_request = fields
        .control()
        .is_management(ManagementSubtype::ProbeRequest);
    let station = receiver == addresses.station
        || group_receiver
            && (transmitter == addresses.station_bssid || bssid == addresses.station_bssid);
    let access_point = receiver == addresses.access_point
        || bssid == addresses.access_point
        || group_receiver && is_group(bssid) && probe_request;
    select(station, access_point)
}

fn classify_data(fields: AddressFields, addresses: StaApRxAddresses) -> StaApRxRoute {
    let Some((transmitter, bssid_or_destination)) = fields.transmitter().zip(fields.address3())
    else {
        return StaApRxRoute::Malformed;
    };
    let receiver = fields.receiver();
    let group_receiver = is_group(receiver);
    match (fields.control().to_ds(), fields.control().from_ds()) {
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

const fn is_group(address: [u8; 6]) -> bool {
    let [first, ..] = address;
    AddressFlags::from_bytes([first]).group()
}

#[cfg(test)]
mod tests;
