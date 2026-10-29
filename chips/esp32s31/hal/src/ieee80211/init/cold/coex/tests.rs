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
fn every_coexistence_priority_reaches_the_mac_unchanged() {
    use crate::{coex::CoexPti, types::MacPti};

    for value in 0..=u8::MAX {
        if let Some(pti) = CoexPti::new(value) {
            let cold = MacCoexPti::from(pti);
            assert_eq!(cold.value(), value);
            assert_eq!(cold.pti(), MacPti::from_coex(pti));
            assert_eq!(cold.pti().get(), u32::from(value));
        }
    }
}
