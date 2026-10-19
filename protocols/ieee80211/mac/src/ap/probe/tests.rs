extern crate std;
use super::*;
use crate::ap::{
    ApManagementRequest, parse_ap_management_request, profile::tests::TEST_ADVERTISEMENT,
};
use crate::sequence::seq;

const AP: [u8; 6] = [2, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];

fn request(ssid: &[u8], directed: bool) -> std::vec::Vec<u8> {
    let mut frame = std::vec![0; 24];
    frame[0] = 0x40;
    frame[4..10].copy_from_slice(if directed { &AP } else { &[0xff; 6] });
    frame[10..16].copy_from_slice(&PEER);
    frame[16..22].fill(0xff);
    frame.extend_from_slice(&[0, ssid.len() as u8]);
    frame.extend_from_slice(ssid);
    frame
}

#[test]
fn accepts_broadcast_and_directed_discovery_without_association() {
    for directed in [false, true] {
        for name in [b"".as_slice(), b"ap"] {
            let frame = request(name, directed);
            assert_eq!(
                parse_ap_management_request(&TEST_ADVERTISEMENT, &frame, AP),
                Some(ApManagementRequest::Probe {
                    peer: PEER,
                    ssid: name
                })
            );
        }
    }
}

#[test]
fn rejects_foreign_addresses_and_malformed_or_ambiguous_requests() {
    let valid = request(b"ap", false);
    let mut cases = std::vec::Vec::new();
    for offset in [4, 16] {
        let mut frame = valid.clone();
        frame[offset..offset + 6].copy_from_slice(&PEER);
        cases.push(frame);
    }
    let mut duplicate = valid.clone();
    duplicate.extend_from_slice(&[0, 0]);
    cases.push(duplicate);
    let mut truncated = valid.clone();
    truncated.extend_from_slice(&[1, 4, 1]);
    cases.push(truncated);
    cases.push(valid[..24].to_vec());
    cases.push(request(&[b'a'; 33], false));
    for (offset, mask) in [(0, 1), (1, 0x40), (1, 4), (22, 1), (10, 1)] {
        let mut frame = valid.clone();
        frame[offset] |= mask;
        cases.push(frame);
    }
    for frame in cases {
        assert!(parse_ap_management_request(&TEST_ADVERTISEMENT, &frame, AP).is_none());
    }
}

#[test]
fn response_preserves_advertisement_without_tim_or_beacon_mutation() {
    use crate::{
        beacon::write_ht_beacon, channel::WifiChannel, security::ApSecurityPolicy, ssid::WifiSsid,
    };
    for security in [
        ApSecurityPolicy::Open,
        ApSecurityPolicy::Wpa2Personal,
        ApSecurityPolicy::Wpa3Personal,
    ] {
        let mut beacon = [0; 256];
        let len = write_ht_beacon(
            &TEST_ADVERTISEMENT,
            &mut beacon,
            AP,
            &WifiSsid::new(b"ap").unwrap(),
            crate::channel::Channel::from_wifi_channel(WifiChannel::mhz20(13).unwrap()),
            100,
            2,
            seq(0),
            security,
            crate::protection::ApBssProtection::default(),
        )
        .unwrap();
        let before = beacon;
        assert!(matches_ssid(&beacon[..len], b""));
        assert!(matches_ssid(&beacon[..len], b"ap"));
        assert!(!matches_ssid(&beacon[..len], b"foreign"));
        let mut output = [0; 256];
        let n = write_response(&beacon[..len], PEER, seq(7), 12345, &mut output).unwrap();
        assert_eq!(beacon, before);
        assert_eq!(output[0], 0x50);
        assert_eq!(&output[4..10], &PEER);
        assert_eq!(&output[10..22], &beacon[10..22]);
        assert_eq!(&output[24..32], &12345_u64.to_le_bytes());
        assert_eq!(&output[32..36], &beacon[32..36]);
        let mut src = &beacon[36..len];
        let mut dst = &output[36..n];
        while !src.is_empty() {
            let length = 2 + usize::from(src[1]);
            if src[0] != 5 {
                assert_eq!(&src[..length], &dst[..length]);
                dst = &dst[length..];
            }
            src = &src[length..];
        }
        assert!(dst.is_empty());
        let mut short = std::vec![0xaa; n - 1];
        assert_eq!(
            write_response(&beacon[..len], PEER, seq(0), 0, &mut short),
            Err(ResponseError::OutputTooSmall { required: n })
        );
        assert!(short.iter().all(|byte| *byte == 0xaa));
    }
}

fn probe_parsers_never_panic(count: usize) {
    use crate::{
        beacon::write_ht_beacon, channel::WifiChannel, security::ApSecurityPolicy, ssid::WifiSsid,
    };
    let mut beacon = [0; 256];
    let len = write_ht_beacon(
        &TEST_ADVERTISEMENT,
        &mut beacon,
        AP,
        &WifiSsid::new(b"ap").unwrap(),
        crate::channel::Channel::from_wifi_channel(WifiChannel::mhz20(13).unwrap()),
        100,
        2,
        seq(0),
        ApSecurityPolicy::Wpa2Personal,
        crate::protection::ApBssProtection::default(),
    )
    .unwrap();
    let beacon = &beacon[..len];
    let broadcast = request(b"", false);
    let directed = request(b"ap", true);
    let seeds: [&[u8]; 4] = [&broadcast[24..], &directed[24..], &beacon[36..], beacon];
    crate::fuzz::inputs(&seeds, count, 160, |bytes| {
        let _ = ssid(bytes);
        // The received SSID against the AP's own advertisement, and received
        // bytes in the advertisement's place.
        let _ = matches_ssid(beacon, bytes);
        let _ = matches_ssid(bytes, b"ap");
    });
}

#[test]
fn probe_parsers_never_panic_on_received_bytes() {
    probe_parsers_never_panic(crate::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn probe_parsers_never_panic_on_received_bytes_full() {
    probe_parsers_never_panic(crate::fuzz::FULL);
}

#[test]
fn an_ssid_needs_complete_elements_and_one_bounded_ssid() {
    assert_eq!(ssid(&[0, 2, b'a', b'p', 1, 1, 0x82]), Some(&b"ap"[..]));
    assert_eq!(ssid(&[0, 0]), Some(&b""[..]));
    for elements in [
        &[][..],
        &[0],
        &[0, 3, b'a', b'p'],
        &[0, 2, b'a', b'p', 1],
        &[0, 1, b'a', 0, 1, b'b'],
        &[1, 1, 0x82],
    ] {
        assert_eq!(ssid(elements), None);
    }
    let mut long = std::vec![0, 33];
    long.extend_from_slice(&[b'a'; 33]);
    assert_eq!(ssid(&long), None);
    assert!(!matches_ssid(&[0; 36], b""));
}
