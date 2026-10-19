//! The access point's parsers of received frames never panic.

use super::*;
use crate::address::{Bssid, IndividualAddress, OwnAddress};
use crate::ap::profile::tests::TEST_ADVERTISEMENT;
use crate::fuzz::{FULL, QUICK, inputs};
use crate::sequence::seq;
use crate::station::{
    OpenAuthenticationRequest, SaeAuthenticationFrame, StaManagementFrame, StaManagementSubtype,
};
use crate::station_power_save::{StaNullDataFrame, StaPowerManagement};
use std::vec::Vec;

const ACCESS_POINT: [u8; 6] = [2, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];

fn source() -> OwnAddress {
    OwnAddress::new(IndividualAddress::new(PEER).unwrap())
}

fn bssid() -> Bssid {
    Bssid::new(IndividualAddress::new(ACCESS_POINT).unwrap()).unwrap()
}

/// A management header from the peer to this access point.
fn to_access_point(frame_control: u16, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::from([0_u8; MANAGEMENT_HEADER_LEN]);
    frame[..2].copy_from_slice(&frame_control.to_le_bytes());
    frame[4..10].copy_from_slice(&ACCESS_POINT);
    frame[10..16].copy_from_slice(&PEER);
    frame[16..22].copy_from_slice(&ACCESS_POINT);
    frame.extend_from_slice(body);
    frame
}

/// Requests a peer sends an access point, from the station's own encoders
/// and the module's tests.
fn management_seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    let mut frame = [0_u8; 128];

    let length = OpenAuthenticationRequest {
        source: source(),
        bssid: bssid(),
        sequence_number: seq(1),
    }
    .encode(&mut frame)
    .unwrap();
    seeds.push(frame[..length].to_vec());

    let length = SaeAuthenticationFrame {
        source: source(),
        bssid: bssid(),
        sequence_number: seq(2),
        transaction: 1,
        status_code: 126,
        body: &[0x13, 0, 0xaa, 0xbb],
    }
    .encode(&mut frame)
    .unwrap();
    seeds.push(frame[..length].to_vec());

    for body in [
        &[3, 1, 7, 0, 0, 0x02, 0x08, 0, 0][..],
        &[3, 2, 0, 0x58, 39, 0][..],
        &[8, 0, 0x12, 0x34][..],
    ] {
        let length = StaManagementFrame {
            subtype: StaManagementSubtype::Action,
            source: source(),
            bssid: bssid(),
            sequence_number: seq(3),
            body,
        }
        .encode(&mut frame)
        .unwrap();
        seeds.push(frame[..length].to_vec());
    }

    // The protected SA Query and Deauthentication behind a CCMP header.
    let ccmp = [7, 0, 0, 0x20, 0, 0, 0, 0];
    let mut query = to_access_point(0x40d0, &ccmp);
    query.extend_from_slice(&[8, 1, 0x12, 0x34]);
    seeds.push(query);
    let mut deauthentication = to_access_point(0x40c0, &ccmp);
    deauthentication.extend_from_slice(&[3, 0]);
    seeds.push(deauthentication);
    seeds.push(to_access_point(0x00a0, &[8, 0]));

    // A broadcast probe request for an SSID with its rates.
    let mut probe = to_access_point(0x0040, &[0, 4, b't', b'e', b's', b't', 1, 2, 0x82, 0x84]);
    probe[4..10].fill(0xff);
    probe[16..22].fill(0xff);
    seeds.push(probe);

    // Association requests: legacy rates and RSN, an RSNXE pair, WMM, a
    // legacy WPA element, and a complete HT40 request.
    let mut legacy = to_access_point(0x0000, &[0x21, 0, 10, 0]);
    legacy.extend_from_slice(&[
        1, 4, 0x82, 0x84, 0x0c, 0x30, 50, 3, 0x48, 0x6c, 0x7f, 48, 1, 0,
    ]);
    seeds.push(legacy);
    let mut rsnxe = to_access_point(0x0000, &[0x10, 0, 10, 0]);
    rsnxe.extend_from_slice(&[48, 1, 0, 244, 1, 0x20, 244, 2, 0x40, 0x00]);
    seeds.push(rsnxe);
    let mut wmm = to_access_point(0x0000, &[0x10, 0, 10, 0]);
    wmm.extend_from_slice(&[221, 7, 0x00, 0x50, 0xf2, 0x02, 0, 1, 0]);
    wmm.extend_from_slice(&[221, 6, 0x00, 0x50, 0xf2, 0x01, 1, 0]);
    seeds.push(wmm);
    let channel =
        crate::channel::Channel::ghz2_4(6, crate::channel::WifiChannelWidth::Mhz40Above).unwrap();
    let mut ht = to_access_point(0x0000, &[0x01, 0, 10, 0]);
    ht.extend_from_slice(&[1, 4, 0x82, 0x84, 0x0c, 0x6c]);
    ht.extend_from_slice(&crate::ht::ht_capability_ie(
        crate::ht::HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01),
        channel,
    ));
    seeds.push(ht);
    seeds
}

fn management(count: usize) {
    let seeds = management_seeds();
    for seed in &seeds {
        assert!(
            parse_ap_management_request(&TEST_ADVERTISEMENT, seed, ACCESS_POINT).is_some(),
            "seed {seed:02x?} parses"
        );
    }
    let seeds: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    inputs(&seeds, count, 128, |bytes| {
        if let Some(request) = parse_ap_management_request(&TEST_ADVERTISEMENT, bytes, ACCESS_POINT)
        {
            match request {
                ApManagementRequest::Association {
                    security,
                    ht_capabilities,
                    ..
                } => {
                    let _ = (
                        security.rsn_ie.map(<[u8]>::len),
                        security.rsnxe.map(<[u8]>::len),
                        ht_capabilities.map(|ht| ht.highest_rx_mcs()),
                    );
                }
                ApManagementRequest::Probe { ssid, .. } => {
                    let _ = ssid.len();
                }
                ApManagementRequest::SaeAuthentication { body, .. } => {
                    let _ = body.len();
                }
                _ => {}
            }
        }
        if let Some(subtype) = ApManagementSubtype::parse(bytes) {
            let _ = subtype.is_robust(bytes.get(MANAGEMENT_HEADER_LEN..).unwrap_or_default());
        }
    });
}

#[test]
fn management_request_parser_never_panics() {
    management(QUICK);
}

#[test]
#[ignore = "the full run"]
fn management_request_parser_never_panics_full() {
    management(FULL);
}

fn power_save(count: usize) {
    let mut ps_poll = [0_u8; 16];
    ps_poll[..2].copy_from_slice(&0x00a4_u16.to_le_bytes());
    ps_poll[2..4].copy_from_slice(&0xc123_u16.to_le_bytes());
    ps_poll[4..10].copy_from_slice(&ACCESS_POINT);
    ps_poll[10..16].copy_from_slice(&PEER);
    let mut data = to_access_point(0x1108, &[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00]);
    data[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 3]);
    let mut null_data = [0_u8; STA_NULL_DATA_FRAME_LEN];
    StaNullDataFrame {
        station_address: source(),
        bssid: bssid(),
        sequence_number: seq(5),
        power_management: StaPowerManagement::PowerSave,
    }
    .encode(&mut null_data)
    .unwrap();
    assert!(observe_ap_power_save_for_access_point(&ps_poll, ACCESS_POINT).is_some());
    assert!(observe_ap_power_save(&data).is_some());
    assert!(observe_ap_null_data_power_save_for_access_point(&null_data, ACCESS_POINT).is_some());
    inputs(&[&ps_poll, &data, &null_data], count, 48, |bytes| {
        let _ = observe_ap_power_save(bytes);
        let _ = observe_ap_power_save_for_access_point(bytes, ACCESS_POINT);
        let _ = observe_ap_null_data_power_save_for_access_point(bytes, ACCESS_POINT);
    });
}

#[test]
fn power_save_observers_never_panic() {
    power_save(QUICK);
}

#[test]
#[ignore = "the full run"]
fn power_save_observers_never_panic_full() {
    power_save(FULL);
}
