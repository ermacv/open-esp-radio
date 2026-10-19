use super::MacCoexEvent;

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
