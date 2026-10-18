use oer_ieee80211_mac::{
    ap::profile::{Advertisement, LegacyRates, WmmParameters},
    extensions::wmm::WmmAcParameters,
    ht::HtLocalCapabilities,
};

use super::*;

/// An advertisement of one-stream HT with the ordinary legacy rates.
const ADVERTISEMENT: Advertisement = match Advertisement::new(
    LegacyRates::new(
        [0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60],
        [0x6c, 0x12, 0x24, 0x48],
    ),
    HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01),
    match WmmParameters::new(
        4,
        false,
        [WmmAcParameters {
            admission_control_mandatory: false,
            aifsn: 3,
            ecw_min: 4,
            ecw_max: 10,
            txop_limit_units_32_us: 0,
        }; 4],
    ) {
        Some(value) => value,
        None => panic!("the WMM parameters fit their fields"),
    },
    0x0421,
) {
    Some(value) => value,
    None => panic!("the advertisement leaves Privacy to the security mode"),
};
#[test]
fn workload_is_finite_and_separates_single_fixed_and_rotating_sources() {
    assert_eq!(request(0).unwrap().offset_us, 1_000_000);
    assert_eq!(request(1).unwrap().source, request(200).unwrap().source);
    let rotating: std::collections::BTreeSet<_> = (201..REQUESTS)
        .map(|i| request(i).unwrap().source)
        .collect();
    assert_eq!(rotating.len(), 200);
    assert!(request(REQUESTS).is_none());
    assert!(request(400).unwrap().offset_us < 7_000_000);
}
#[test]
fn generated_frames_are_accepted_by_the_production_probe_parser() {
    let config = Config {
        ssid: "test".into(),
        channel: 13,
    };
    let bssid = [2, 0, 0, 0, 0, 1];
    for i in [0, 1, 200, 201, 400] {
        let r = request(i).unwrap();
        let frame = super::super::frame::encode(&config, bssid, r);
        assert_eq!(
            oer_ieee80211_mac::ap::parse_ap_management_request(&ADVERTISEMENT, &frame[9..], bssid),
            Some(oer_ieee80211_mac::ap::ApManagementRequest::Probe {
                peer: r.source,
                ssid: b"test"
            })
        );
        assert_eq!(&frame[9 + 10..9 + 16], &r.source);
    }
}
#[test]
fn partial_submission_and_missed_pacing_are_not_success() {
    let mut r = Report {
        bssid: [2, 0, 0, 0, 0, 1],
        submitted: REQUESTS,
        ..Default::default()
    };
    assert!(r.validate().is_ok());
    r.maximum_lateness_us = MAX_LATENESS_US + 1;
    assert!(r.validate().is_err());
    r.maximum_lateness_us = 0;
    r.submitted -= 1;
    assert!(r.validate().is_err());
}
