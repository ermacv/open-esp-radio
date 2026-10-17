use super::{
    WMM_OUI_AND_TYPE, WMM_PARAMETER_BODY_LEN, WmmAccessCategory, parse_wmm_parameter_element,
};

const STANDARD_PARAMETER_ELEMENT: [u8; 26] = [
    221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42, 0x43,
    94, 0, 0x72, 0x32, 47, 0,
];

#[test]
fn parses_all_four_access_categories_and_complete_txop() {
    let parameters = parse_wmm_parameter_element(&STANDARD_PARAMETER_ELEMENT).unwrap();
    assert_eq!(parameters.parameter_set_count, 5);
    assert!(parameters.uapsd);

    let best_effort = parameters.access_category(WmmAccessCategory::BestEffort);
    assert_eq!(best_effort.aifsn, 3);
    assert_eq!(best_effort.ecw_min, 4);
    assert_eq!(best_effort.ecw_max, 10);
    assert_eq!(best_effort.txop_limit_units_32_us, 0);

    let video = parameters.access_category(WmmAccessCategory::Video);
    assert_eq!(video.aifsn, 2);
    assert_eq!(video.ecw_min, 3);
    assert_eq!(video.ecw_max, 4);
    assert_eq!(video.txop_limit_units_32_us, 94);

    let voice = parameters.access_category(WmmAccessCategory::Voice);
    assert!(voice.admission_control_mandatory);
    assert_eq!(voice.txop_limit_units_32_us, 47);
}

#[test]
fn retains_the_standard_txop_high_byte_that_the_blob_drops() {
    let mut element = STANDARD_PARAMETER_ELEMENT;
    element[21] = 1;
    assert_eq!(
        parse_wmm_parameter_element(&element)
            .unwrap()
            .access_category(WmmAccessCategory::Video)
            .txop_limit_units_32_us,
        350
    );
}

#[test]
fn rejects_wrong_identity_short_elements_and_duplicate_aci() {
    let mut element = STANDARD_PARAMETER_ELEMENT;
    element[2..6].copy_from_slice(&WMM_OUI_AND_TYPE);
    element[1] = WMM_PARAMETER_BODY_LEN - 1;
    assert!(parse_wmm_parameter_element(&element).is_none());

    element = STANDARD_PARAMETER_ELEMENT;
    element[22] = 0x42;
    assert!(parse_wmm_parameter_element(&element).is_none());
}
