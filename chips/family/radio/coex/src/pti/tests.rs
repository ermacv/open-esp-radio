use super::{CoexEventId, CoexPti, CoexPtiTable};

fn event(value: u8) -> CoexEventId {
    CoexEventId::new(value).unwrap_or_else(|| panic!("event {value} is in the vendor table"))
}

/// The cold Wi-Fi MAC events and the PHY grant-protect event carry the
/// vendor priorities, and every entry fits the four-bit hardware field.
#[test]
fn the_vendor_table_holds_the_cold_mac_and_grant_protect_priorities() {
    let table = CoexPtiTable::VENDOR;
    for (value, pti) in [(1, 5), (3, 7), (10, 3), (15, 1), (48, 15)] {
        assert_eq!(table.pti(event(value)).value(), pti);
    }
}

#[test]
fn events_and_priorities_outside_their_domains_are_rejected() {
    assert!(CoexEventId::new(49).is_none());
    assert!(CoexPti::new(0x10).is_none());
}

#[test]
fn every_raw_event_and_priority_is_admitted_exactly_within_its_domain() {
    for value in 0..=u8::MAX {
        assert_eq!(CoexEventId::new(value).is_some(), value < 49);
        if let Some(event) = CoexEventId::new(value) {
            assert_eq!(event.value(), value);
        }
        assert_eq!(CoexPti::new(value).is_some(), value < 16);
    }
}
