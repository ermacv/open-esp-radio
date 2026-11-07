#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects
)]

use super::{CoexPti, ieee802154_event};
use oer_chips_family_radio_coex::CoexPtiTable;
use oer_protocols_ieee802154_port::RadioOperation;

/// `ieee802154_mac_init` and `ieee802154_set_txrx_pti` read the idle scene,
/// immediate and timed operations and the ACK from events 44, 43, 42 and 42
/// of the shared table.
#[test]
fn ieee802154_operations_read_their_shared_table_events() {
    let table = CoexPtiTable::VENDOR;
    for (operation, pti) in [
        (RadioOperation::Idle, 1),
        (RadioOperation::Tx, 3),
        (RadioOperation::Rx, 3),
        (RadioOperation::TxAt, 8),
        (RadioOperation::RxAt, 8),
        (RadioOperation::Ack, 8),
        (RadioOperation::CslSlot, 8),
    ] {
        assert_eq!(table.pti(ieee802154_event(operation)).value(), pti);
    }
}

/// A priority change reaches every operation that reads the changed event.
#[test]
fn a_changed_event_reaches_the_operations_that_read_it() {
    let mut table = CoexPtiTable::VENDOR;
    let event = ieee802154_event(RadioOperation::Rx);
    table.set(
        event,
        CoexPti::new(5).unwrap_or_else(|| panic!("5 is a priority")),
    );
    assert_eq!(table.pti(ieee802154_event(RadioOperation::Tx)).value(), 5);
    assert_eq!(table.pti(ieee802154_event(RadioOperation::Rx)).value(), 5);
    assert_eq!(table.pti(ieee802154_event(RadioOperation::TxAt)).value(), 8);
}
