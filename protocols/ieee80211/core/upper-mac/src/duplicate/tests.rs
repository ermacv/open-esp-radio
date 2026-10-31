use super::*;

#[test]
fn duplicate_history_query_never_accepts_fragment_state() {
    let mut filter = RxDuplicateFilter::new();
    assert!(!filter.is_duplicate(false, 0x1230, None));
    let ordinary_history = filter;

    assert!(filter.is_known_duplicate(true, 0x1230, None));
    assert!(!filter.is_known_duplicate(true, 0x1240, None));
    assert!(!filter.is_known_duplicate(false, 0x1230, None));
    assert_eq!(filter, ordinary_history);
}

#[test]
fn sta_rx_duplicate_filter_requires_retry_and_matching_sequence_space() {
    let mut filter = RxDuplicateFilter::new();
    assert!(!filter.is_duplicate(false, 0x1230, None));
    assert!(filter.is_duplicate(true, 0x1230, None));
    assert!(!filter.is_duplicate(false, 0x1230, None));
    assert!(!filter.is_duplicate(true, 0x1240, None));

    assert!(!filter.is_duplicate(false, 0x2000, Some(3)));
    assert!(!filter.is_duplicate(true, 0x2000, Some(4)));
    assert!(filter.is_duplicate(true, 0x2000, Some(3)));
}

/// Each four octets are one MPDU's Retry, Sequence Control and TID (none
/// when the fourth octet's top bit is set).
fn run_duplicate(count: usize) {
    let seed = [
        0, 0x30, 0x12, 0, 1, 0x30, 0x12, 0, 1, 0x30, 0x12, 0x80, 0, 0x40, 0x12, 17,
    ];
    oer_ieee80211_wire::fuzz::inputs(&[&seed], count, 128, |bytes| {
        let mut filter = RxDuplicateFilter::new();
        for &[retry, low, high, tid] in bytes.as_chunks::<4>().0 {
            let retry = retry & 1 != 0;
            let sequence_control = u16::from_le_bytes([low, high]);
            let tid = (tid & 0x80 == 0).then_some(tid);
            let known = filter.is_known_duplicate(retry, sequence_control, tid);
            let duplicate = filter.is_duplicate(retry, sequence_control, tid);
            assert_eq!(known, duplicate);
            assert!(filter.is_known_duplicate(true, sequence_control, tid));
        }
    });
}

#[test]
fn duplicate_filter_never_panics() {
    run_duplicate(oer_ieee80211_wire::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn duplicate_filter_never_panics_full() {
    run_duplicate(oer_ieee80211_wire::fuzz::FULL);
}
