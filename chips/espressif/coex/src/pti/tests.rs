#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects
)]

use super::{CoexEventId, CoexPti, CoexPtiTable};

fn event(value: u8) -> CoexEventId {
    CoexEventId::new(value).unwrap_or_else(|| panic!("event {value} is in the vendor table"))
}

/// The cold Wi-Fi MAC events and the PHY grant-protect event carry the
/// vendor priorities, and every entry fits the four-bit hardware field.
#[test]
fn the_vendor_table_holds_the_cold_mac_and_grant_protect_priorities() {
    let table = CoexPtiTable::VENDOR;
    assert!(table.as_bytes().iter().all(|value| *value <= 0x0f));
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
fn setting_a_priority_changes_only_its_event() {
    let mut table = CoexPtiTable::VENDOR;
    table.set(
        event(47),
        CoexPti::new(3).unwrap_or_else(|| panic!("3 is a priority")),
    );
    assert_eq!(table.pti(event(47)).value(), 3);
    assert_eq!(table.pti(event(48)).value(), 15);
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

#[test]
fn every_event_projects_to_its_own_priority_entry() {
    for value in 0..49 {
        for priority in 0..16 {
            let mut table = CoexPtiTable::VENDOR;
            table.set(event(value), CoexPti::new(priority).unwrap());
            assert_eq!(table.pti(event(value)).value(), priority);
            for (other, (&before, &after)) in CoexPtiTable::VENDOR
                .as_bytes()
                .iter()
                .zip(table.as_bytes())
                .enumerate()
            {
                if other != usize::from(value) {
                    assert_eq!(after, before);
                }
            }
        }
    }
}
