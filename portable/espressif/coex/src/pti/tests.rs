use super::{CoexEventId, CoexPti, CoexPtiTable, Ieee802154CoexLevel};
use oer_radio_coex::CoexPriority;

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

/// `coex_ieee802154_pti_get` reads IEEE 802.15.4 levels from events 41
/// through 44 of the shared table.
#[test]
fn ieee802154_levels_read_their_shared_table_events() {
    let table = CoexPtiTable::VENDOR;
    for (level, pti) in [
        (Ieee802154CoexLevel::High, 12),
        (Ieee802154CoexLevel::Middle, 8),
        (Ieee802154CoexLevel::Low, 3),
        (Ieee802154CoexLevel::Idle, 1),
    ] {
        assert_eq!(table.ieee802154_pti(level).value(), pti);
    }
    // A priority change reaches the level that reads the changed event.
    let mut table = table;
    table.set(
        event(43),
        CoexPti::new(5).unwrap_or_else(|| panic!("5 is a priority")),
    );
    assert_eq!(table.ieee802154_pti(Ieee802154CoexLevel::Low).value(), 5);
}

/// Every portable priority selects one level and returns from it, and a
/// higher portable priority reads a vendor priority at least as high.
#[test]
fn portable_priorities_map_onto_the_ieee802154_levels_in_order() {
    let table = CoexPtiTable::VENDOR;
    for priority in CoexPriority::ALL {
        assert_eq!(Ieee802154CoexLevel::from(priority).priority(), priority);
    }
    let ptis = CoexPriority::ALL.map(|priority| table.ieee802154_pti(priority.into()));
    assert!(ptis.windows(2).all(|pair| pair[0] <= pair[1]));
}
