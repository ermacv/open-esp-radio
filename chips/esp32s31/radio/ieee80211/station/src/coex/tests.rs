#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects
)]

use super::wifi_event;
use oer_chips_family_radio_coex::wifi_events;
use oer_protocols_ieee80211_contracts_port::{ConnectionFrame, Operation, TxOperation};

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
