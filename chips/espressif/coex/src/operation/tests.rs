use super::{ieee802154_event, wifi_event};
use crate::{CoexPtiTable, wifi_events};
use oer_ieee80211_lower_mac::{ConnectionFrame, Operation, TxOperation};
use oer_ieee802154_mac::RadioOperation;

#[test]
fn wifi_rows_name_their_events_and_data_takes_none() {
    assert_eq!(
        wifi_event(Operation::BeaconWindow),
        Some(wifi_events::BEACON)
    );
    assert_eq!(wifi_event(Operation::Transmit(TxOperation::Data)), None);
    assert_eq!(
        wifi_event(Operation::Transmit(TxOperation::Connection(
            ConnectionFrame::ProbeRequest
        ))),
        Some(wifi_events::PROBE_REQUEST)
    );
    for frame in [
        ConnectionFrame::Authentication,
        ConnectionFrame::Association,
        ConnectionFrame::Eapol,
    ] {
        assert_eq!(
            wifi_event(Operation::Transmit(TxOperation::Connection(frame))),
            Some(wifi_events::CONNECTION)
        );
    }
}

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
        assert_eq!(table.ieee802154_pti(operation).value(), pti);
    }
}

/// A priority change reaches every operation that reads the changed event.
#[test]
fn a_changed_event_reaches_the_operations_that_read_it() {
    let mut table = CoexPtiTable::VENDOR;
    let event = ieee802154_event(RadioOperation::Rx);
    table.set(
        event,
        crate::CoexPti::new(5).unwrap_or_else(|| panic!("5 is a priority")),
    );
    assert_eq!(table.ieee802154_pti(RadioOperation::Tx).value(), 5);
    assert_eq!(table.ieee802154_pti(RadioOperation::Rx).value(), 5);
    assert_eq!(table.ieee802154_pti(RadioOperation::TxAt).value(), 8);
}
