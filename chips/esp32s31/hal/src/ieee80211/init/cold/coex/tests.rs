use super::{MacCoexEvent, MacCoexPti};

#[test]
fn cold_queries_read_their_own_shared_coexistence_events() {
    for (query, event) in [
        (MacCoexEvent::Event1, 1),
        (MacCoexEvent::Event3, 3),
        (MacCoexEvent::Event10, 10),
        (MacCoexEvent::Event15, 15),
    ] {
        assert_eq!(query.coex_event().value(), event);
    }
}

#[test]
fn a_priority_above_four_bits_is_not_a_mac_priority() {
    assert_eq!(MacCoexPti::new(15).map(MacCoexPti::value), Some(15));
    assert!(MacCoexPti::new(16).is_none());
}
