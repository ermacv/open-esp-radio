//! Receive routing for one same-channel STA plus SoftAP pair.
//!
//! The classifier interprets only public IEEE 802.11 header fields. It does
//! not infer association, authorization, key ownership or hardware policy;
//! role-local consumers must still validate those semantics after routing.
//!
//! Received bytes reach this module; nothing here panics on them.

use oer_protocols_ieee802_wire::{Eui48, Eui48Kind};
use oer_protocols_ieee80211_wire::frame::{
    AddressFields, AddressFieldsError, FrameType, ManagementSubtype,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaApRxAddresses {
    pub station: Eui48,
    pub station_bssid: Eui48,
    pub access_point: Eui48,
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
/// The addressee a frame's address test names.
macro_rules! addressee {
    ($addressed:expr) => {
        if $addressed {
            Addressee::Addressed
        } else {
            Addressee::Other
        }
    };
}

pub fn classify_sta_ap_rx(mpdu: &[u8], addresses: StaApRxAddresses) -> StaApRxRoute {
    let fields = match AddressFields::parse(mpdu) {
        Ok(fields) => fields,
        // An address layout the codec does not know proves no interface.
        Err(AddressFieldsError::UnknownLayout) => return StaApRxRoute::Foreign,
        Err(AddressFieldsError::Truncated | AddressFieldsError::UnknownVersion) => {
            return StaApRxRoute::Malformed;
        }
    };
    let receiver = fields.receiver();
    match fields.control().kind() {
        FrameType::Control => select(
            addressee!(receiver == addresses.station),
            addressee!(receiver == addresses.access_point),
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
    select(addressee!(station), addressee!(access_point))
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
            addressee!(
                receiver == addresses.station
                    || group_receiver && transmitter == addresses.station_bssid
            ),
            Addressee::Other,
        ),
        (true, false) => select(
            Addressee::Other,
            addressee!(receiver == addresses.access_point),
        ),
        (false, false) => select(
            addressee!(
                receiver == addresses.station
                    || group_receiver && bssid_or_destination == addresses.station_bssid
            ),
            addressee!(
                receiver == addresses.access_point
                    || group_receiver && bssid_or_destination == addresses.access_point
            ),
        ),
        (true, true) => StaApRxRoute::Foreign,
    }
}

/// Whether a frame's addresses name one interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Addressee {
    Addressed,
    Other,
}

const fn select(station: Addressee, access_point: Addressee) -> StaApRxRoute {
    match (station, access_point) {
        (Addressee::Addressed, Addressee::Other) => StaApRxRoute::Interface(StaApVif::Station),
        (Addressee::Other, Addressee::Addressed) => StaApRxRoute::Interface(StaApVif::AccessPoint),
        (Addressee::Other, Addressee::Other) => StaApRxRoute::Foreign,
        (Addressee::Addressed, Addressee::Addressed) => StaApRxRoute::Ambiguous,
    }
}

const fn is_group(address: Eui48) -> bool {
    matches!(address.kind(), Eui48Kind::Group(_))
}

#[cfg(test)]
mod tests;
