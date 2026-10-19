use super::{EdcaBackoffState, EdcaContentionParameters, EdcaParametersError, EdcaQueues};
use crate::tx::LegacyTxQueue;
use oer_ieee80211_mac::extensions::wmm::parse_wmm_parameter_element;

const STANDARD_WMM: [u8; 26] = [
    221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42, 0x43,
    94, 0, 0x72, 0x32, 47, 0,
];

#[test]
fn vendor_defaults_match_all_four_lmac_init_records() {
    let queues = EdcaQueues::vendor_defaults();
    assert_eq!(
        queues.queue(LegacyTxQueue::Voice).parameters(),
        EdcaContentionParameters::new(2, 2, 3).unwrap()
    );
    assert_eq!(
        queues.queue(LegacyTxQueue::Video).parameters(),
        EdcaContentionParameters::new(2, 3, 4).unwrap()
    );
    assert_eq!(
        queues.queue(LegacyTxQueue::BestEffort).parameters(),
        EdcaContentionParameters::new(3, 4, 10).unwrap()
    );
    assert_eq!(
        queues.queue(LegacyTxQueue::Background).parameters(),
        EdcaContentionParameters::new(7, 4, 10).unwrap()
    );
}

#[test]
fn retry_expands_cw_to_maximum_and_success_restores_minimum() {
    let mut state = EdcaBackoffState::new(EdcaContentionParameters::new(3, 4, 6).unwrap());
    assert_eq!(state.select_slot(u32::MAX), 15);
    state.record_retry_failure();
    assert_eq!(state.current_exponent(), 5);
    assert_eq!(state.select_slot(u32::MAX), 31);
    state.record_retry_failure();
    state.record_retry_failure();
    assert_eq!(state.current_exponent(), 6);
    assert_eq!(state.select_slot(u32::MAX), 63);
    state.record_success();
    assert_eq!(state.current_exponent(), 4);
}

#[test]
fn reconfigure_clamps_current_to_both_new_bounds() {
    let mut state = EdcaBackoffState::new(EdcaContentionParameters::new(3, 4, 10).unwrap());
    state.record_retry_failure();
    state.record_retry_failure();
    assert_eq!(state.current_exponent(), 6);
    state.reconfigure(EdcaContentionParameters::new(2, 2, 4).unwrap());
    assert_eq!(state.current_exponent(), 4);
    state.reconfigure(EdcaContentionParameters::new(2, 7, 9).unwrap());
    assert_eq!(state.current_exponent(), 7);
}

#[test]
fn wmm_update_is_validated_before_any_queue_changes() {
    let mut queues = EdcaQueues::vendor_defaults();
    let before = queues;
    let mut invalid = STANDARD_WMM;
    invalid[11] = (11 << 4) | 4;
    let parameters = parse_wmm_parameter_element(&invalid).unwrap();
    assert_eq!(
        queues.configure_from_wmm(parameters),
        Err(EdcaParametersError::MaximumExponentOutOfRange(11))
    );
    assert_eq!(queues, before);

    queues
        .configure_from_wmm(parse_wmm_parameter_element(&STANDARD_WMM).unwrap())
        .unwrap();
    assert_eq!(
        queues.queue(LegacyTxQueue::BestEffort).parameters(),
        EdcaContentionParameters::new(3, 4, 10).unwrap()
    );
    let voice = queues.access_policy(LegacyTxQueue::Voice);
    assert!(voice.admission_control_mandatory());
    assert_eq!(voice.txop_limit_units_32_us(), 47);
    let video = queues.access_policy(LegacyTxQueue::Video);
    assert!(!video.admission_control_mandatory());
    assert_eq!(video.txop_limit_units_32_us(), 94);
}

#[test]
fn vendor_defaults_grant_no_implicit_admission_or_txop_ownership() {
    let queues = EdcaQueues::vendor_defaults();
    for queue in [
        LegacyTxQueue::Voice,
        LegacyTxQueue::Video,
        LegacyTxQueue::BestEffort,
        LegacyTxQueue::Background,
    ] {
        let policy = queues.access_policy(queue);
        assert!(!policy.admission_control_mandatory());
        assert_eq!(policy.txop_limit_units_32_us(), 0);
    }
}

#[test]
fn rejects_values_wider_than_the_queue_slot_field() {
    assert_eq!(
        EdcaContentionParameters::new(3, 4, 11),
        Err(EdcaParametersError::MaximumExponentOutOfRange(11))
    );
    assert_eq!(
        EdcaContentionParameters::new(3, 5, 4),
        Err(EdcaParametersError::InvertedExponentRange {
            minimum: 5,
            maximum: 4,
        })
    );
}
