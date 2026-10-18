//! The station's parsers of received frames never panic.

use super::*;
use crate::address::{Bssid, IndividualAddress, OwnAddress};
use crate::fuzz::{FULL, QUICK, inputs};
use crate::sequence::seq;
use std::vec::Vec;

const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];

/// A management header from the access point to this station.
fn from_access_point(frame_control: u16, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::from([0_u8; MANAGEMENT_HEADER_LEN]);
    frame[..2].copy_from_slice(&frame_control.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame.extend_from_slice(body);
    frame
}

/// The access point's SAE Commit and Confirm, as the AP encoder writes them.
fn sae_seeds() -> Vec<Vec<u8>> {
    [
        (1, 126, &[0x13, 0, 0xaa, 0xbb][..]),
        (2, 0, &[1, 0, 0xcc][..]),
    ]
    .into_iter()
    .map(|(transaction, status, body)| {
        let mut frame = [0_u8; 64];
        let length = crate::ap::write_sae_authentication(
            &mut frame,
            BSSID,
            LOCAL,
            transaction,
            status,
            body,
            seq(2),
        )
        .unwrap();
        frame[..length].to_vec()
    })
    .collect()
}

fn authentication(count: usize) {
    let mut open = [0_u8; 64];
    let length =
        crate::ap::write_open_authentication_response(&mut open, BSSID, LOCAL, 17, seq(3)).unwrap();
    let mut seeds = sae_seeds();
    seeds.push(open[..length].to_vec());
    seeds.push(from_access_point(
        OPEN_AUTHENTICATION_FRAME_CONTROL,
        &[0, 0, 2, 0, 0, 0],
    ));
    let seeds: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    for seed in &seeds[..2] {
        assert!(parse_sae_authentication(seed, LOCAL, BSSID).is_some());
    }
    for seed in &seeds[2..] {
        assert!(parse_open_authentication_response(seed, LOCAL, BSSID).is_some());
    }
    inputs(&seeds, count, 96, |bytes| {
        if let Some(sae) = parse_sae_authentication(bytes, LOCAL, BSSID) {
            let _ = (sae.transaction, sae.status_code, sae.body.len());
        }
        if let Some(response) = parse_open_authentication_response(bytes, LOCAL, BSSID) {
            let _ = response.status_code;
        }
    });
}

#[test]
fn authentication_parsers_never_panic() {
    authentication(QUICK);
}

#[test]
#[ignore = "the full run"]
fn authentication_parsers_never_panic_full() {
    authentication(FULL);
}

fn disconnect(count: usize) {
    let local = OwnAddress::new(IndividualAddress::new(LOCAL).unwrap());
    let bssid = Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap();
    let mut deauthentication = [0_u8; 40];
    let length = crate::ap::write_ap_peer_disconnect(
        &mut deauthentication,
        BSSID,
        LOCAL,
        crate::ap::ApPeerDisconnectKind::Deauthentication,
        7,
        seq(4),
    )
    .unwrap();
    let disassociation = from_access_point(DISASSOCIATION_FRAME_CONTROL, &[8, 0]);
    let mut broadcast = from_access_point(DEAUTHENTICATION_FRAME_CONTROL, &[3, 0]);
    broadcast[4..10].fill(0xff);
    let seeds: [&[u8]; 3] = [&deauthentication[..length], &disassociation, &broadcast];
    for seed in seeds {
        assert!(parse_sta_disconnect(seed, local, bssid).is_some());
    }
    inputs(&seeds, count, 64, |bytes| {
        if let Some(disconnect) = parse_sta_disconnect(bytes, local, bssid) {
            let _ = (disconnect.kind, disconnect.reason_code);
        }
    });
}

#[test]
fn disconnect_parser_never_panics() {
    disconnect(QUICK);
}

#[test]
#[ignore = "the full run"]
fn disconnect_parser_never_panics_full() {
    disconnect(FULL);
}

fn association_response(count: usize) {
    let mut ht = from_access_point(
        ASSOCIATION_RESPONSE_FRAME_CONTROL,
        &[0x31, 0x04, 0, 0, 0x2a, 0xc0],
    );
    ht.extend_from_slice(&[
        45, 26, 0x20, 0, 0, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0,
    ]);
    let mut wmm = from_access_point(ASSOCIATION_RESPONSE_FRAME_CONTROL, &[0, 0, 0, 0, 7, 0]);
    wmm.extend_from_slice(&[
        221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 3, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42,
        0x43, 94, 0, 0x62, 0x32, 47, 0,
    ]);
    // A temporary refusal naming its association comeback time, and HE.
    let mut comeback =
        from_access_point(ASSOCIATION_RESPONSE_FRAME_CONTROL, &[0x11, 0, 30, 0, 0, 0]);
    comeback.extend_from_slice(&[56, 5, 3, 0xe8, 0x03, 0, 0, 255, 2, 35, 0, 255, 2, 36, 0]);
    for seed in [&ht, &wmm, &comeback] {
        assert!(parse_association_response(seed, LOCAL, BSSID).is_some());
    }
    assert_eq!(
        parse_association_response(&comeback, LOCAL, BSSID)
            .unwrap()
            .association_comeback_tu,
        Some(1000)
    );
    inputs(&[&ht, &wmm, &comeback], count, 128, |bytes| {
        if let Some(response) = parse_association_response(bytes, LOCAL, BSSID) {
            let _ = (
                response.matches_security(crate::security::LinkProtection::Ccmp),
                response.association_id,
                response.wmm_parameters.map(|set| set.parameter_set_count),
                response.association_comeback_tu,
            );
        }
    });
}

#[test]
fn association_response_parser_never_panics() {
    association_response(QUICK);
}

#[test]
#[ignore = "the full run"]
fn association_response_parser_never_panics_full() {
    association_response(FULL);
}
