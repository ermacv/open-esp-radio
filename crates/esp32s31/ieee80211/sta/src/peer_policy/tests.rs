use super::*;
use oer_esp32s31_ieee80211_mac::rate::schedule::RateScheduleKind;
use oer_ieee80211_mac::extensions::wmm::parse_wmm_parameter_element;

const HE20_MCS9_CAPABILITY: [u8; 24] = [
    255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
    0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
];
const HE20_OPERATION: [u8; 9] = [255, 7, 36, 0, 0, 0, 5, 0xfd, 0xff];
const STANDARD_WMM: [u8; 26] = [
    221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42, 0x43,
    94, 0, 0x72, 0x32, 47, 0,
];

fn he20_access_point() -> ScanRecord {
    let mut access_point = ScanRecord::EMPTY;
    access_point.rssi = -45;
    access_point.ht_capability_ie_present = true;
    access_point.ht_capability_ie[4] = 0x17;
    access_point.he_capability_ie[..HE20_MCS9_CAPABILITY.len()]
        .copy_from_slice(&HE20_MCS9_CAPABILITY);
    access_point.he_capability_ie_len = HE20_MCS9_CAPABILITY.len() as u8;
    access_point.he_operation_ie[..HE20_OPERATION.len()].copy_from_slice(&HE20_OPERATION);
    access_point.he_operation_ie_len = HE20_OPERATION.len() as u8;
    access_point
}

#[test]
fn he20_plan_joins_one_peer_view_without_vendor_layout() {
    let access_point = he20_access_point();
    let scan = StaPeerScanPolicy::new(&access_point).unwrap();
    assert_eq!(scan.ht_ampdu.maximum_aggregate_bytes(), u16::MAX);
    assert_eq!(scan.he_bss_color, 5);

    let response = AssociationResponse {
        capability_info: 0,
        status_code: 0,
        association_id: 7,
        ht_capability: true,
        he_capability: true,
        he_operation: true,
        wmm: true,
        wmm_parameters: Some(parse_wmm_parameter_element(&STANDARD_WMM).unwrap()),
        association_comeback_tu: None,
    };
    let plan = scan
        .complete(&access_point, &response, PhyMode::He20, -95)
        .unwrap();
    assert_eq!(plan.link_metric.value(), 50);
    assert_eq!(plan.wmm.source(), StaWmmSource::AssociationResponse);
    assert_eq!(
        plan.he_capabilities.unwrap().dcm_receive_constellation(),
        HeDcmConstellation::Qam16
    );
    assert!(
        plan.he_peer_state
            .unwrap()
            .extended_range_single_user_permitted()
    );
    assert_eq!(
        plan.rate_control.current_schedule().kind,
        RateScheduleKind::Dot11Ax
    );
}

#[test]
fn response_wmm_overrides_the_scan_parameter_set() {
    let mut access_point = he20_access_point();
    access_point.wmm_ie[..STANDARD_WMM.len()].copy_from_slice(&STANDARD_WMM);
    access_point.wmm_ie_len = STANDARD_WMM.len() as u8;
    let scan = StaPeerScanPolicy::new(&access_point).unwrap();
    assert_eq!(scan.wmm.source(), StaWmmSource::Scan);

    let mut wider = STANDARD_WMM;
    wider[12..14].copy_from_slice(&300_u16.to_le_bytes());
    let response = AssociationResponse {
        capability_info: 0,
        status_code: 0,
        association_id: 7,
        ht_capability: true,
        he_capability: true,
        he_operation: true,
        wmm: true,
        wmm_parameters: Some(parse_wmm_parameter_element(&wider).unwrap()),
        association_comeback_tu: None,
    };
    let plan = scan
        .complete(&access_point, &response, PhyMode::He20, -95)
        .unwrap();
    assert_eq!(plan.wmm.source(), StaWmmSource::AssociationResponse);
}

#[test]
fn rejected_association_cannot_produce_a_peer_plan() {
    let access_point = he20_access_point();
    let response = AssociationResponse {
        capability_info: 0,
        status_code: 17,
        association_id: 0,
        ht_capability: false,
        he_capability: false,
        he_operation: false,
        wmm: false,
        wmm_parameters: None,
        association_comeback_tu: None,
    };
    assert_eq!(
        StaPeerScanPolicy::new(&access_point)
            .unwrap()
            .complete(&access_point, &response, PhyMode::He20, -95)
            .unwrap_err(),
        StaPeerAssociationPlanError::AssociationRejected(17)
    );
}

#[test]
fn scan_policy_collects_every_bss_protection_fact() {
    let mut access_point = ScanRecord::EMPTY;
    access_point.capability_info = 0x0421;
    access_point.supported_rates = [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24];
    access_point.supported_rates_len = 8;
    access_point.erp_information = Some(0x07);
    access_point.ht_operation_ie[..2].copy_from_slice(&[61, 22]);
    access_point.ht_operation_ie[4] = 0x03;
    access_point.ht_operation_ie_present = true;

    let protection = StaPeerScanPolicy::new(&access_point).unwrap().protection;
    assert_eq!(protection.erp, ErpProtection::new(true, true));
    assert_eq!(protection.ht, HtProtectionMode::NonHtMixed);
    assert!(protection.short_preamble);
    assert_eq!(
        protection.basic_rates,
        BasicRates::from_rate_elements(&[0x82, 0x84, 0x8b, 0x96], &[])
    );
    assert_eq!(protection.he_txop_rts_threshold, None);
}
