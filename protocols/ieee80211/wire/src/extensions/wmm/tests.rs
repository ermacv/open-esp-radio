use super::{
    WMM_OUI_AND_TYPE, WMM_PARAMETER_BODY_LEN, WmmAccessCategory, WmmParameterSet,
    parse_wmm_parameter_element,
};

const STANDARD_PARAMETER_ELEMENT: [u8; 26] = [
    221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42, 0x43,
    94, 0, 0x72, 0x32, 47, 0,
];

#[test]
fn the_standard_default_is_the_records_of_table_9_155() {
    // The element above advertises the Table 9-155 contention for an OFDM
    // PHY; its Voice record also sets admission control.
    let advertised = parse_wmm_parameter_element(&STANDARD_PARAMETER_ELEMENT).unwrap();
    let contention = |record: super::WmmAcParameters| {
        (
            record.aifsn,
            record.ecw_min,
            record.ecw_max,
            record.txop_limit_units_32_us,
        )
    };
    for category in WmmAccessCategory::ALL {
        let default = WmmParameterSet::STANDARD_DEFAULT.access_category(category);
        assert!(!default.admission_control_mandatory);
        assert_eq!(
            contention(default),
            contention(advertised.access_category(category))
        );
        let dcf = WmmParameterSet::DCF.access_category(category);
        assert_eq!(contention(dcf), (2, 4, 10, 0));
    }
}

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

fn parse_wmm_parameter_element_never_panics(count: usize) {
    let mut long = STANDARD_PARAMETER_ELEMENT.to_vec();
    long[1] = 28;
    long.extend_from_slice(&[0xdd; 4]);
    let seeds: [&[u8]; 2] = [&STANDARD_PARAMETER_ELEMENT, &long];
    crate::fuzz::inputs(&seeds, count, 64, |element| {
        if let Some(parameters) = parse_wmm_parameter_element(element) {
            for category in [
                WmmAccessCategory::BestEffort,
                WmmAccessCategory::Background,
                WmmAccessCategory::Video,
                WmmAccessCategory::Voice,
            ] {
                let _ = parameters.access_category(category);
            }
        }
    });
}

#[test]
fn parse_wmm_parameter_element_never_panics_on_received_bytes() {
    parse_wmm_parameter_element_never_panics(crate::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn parse_wmm_parameter_element_never_panics_on_received_bytes_full() {
    parse_wmm_parameter_element_never_panics(crate::fuzz::FULL);
}
