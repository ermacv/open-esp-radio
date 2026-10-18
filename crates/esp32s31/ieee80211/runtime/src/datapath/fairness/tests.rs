use super::*;

#[test]
fn rx_serviced_while_no_network_tx_waits_leaves_no_debt() {
    let mut fairness = RxTxFairness::new();
    // A TCP download: thousands of RX frames between its ACKs.
    for _ in 0..3_000 {
        fairness.charge_rx(16, false);
    }
    assert_eq!(fairness.deficit(), 0);
    assert!(!fairness.network_turn_owed());
    // Its next ACK does not throttle RX to one frame per turn.
    assert_eq!(
        fairness.rx_protocol_frame_budget(true),
        Some(RX_TX_FAIRNESS_QUANTUM_FRAMES as usize)
    );
}

#[test]
fn rx_yields_one_transaction_per_quantum_while_both_sides_are_backlogged() {
    let mut fairness = RxTxFairness::new();
    fairness.charge_rx(5, true);
    assert!(!fairness.network_turn_owed());
    assert_eq!(fairness.rx_protocol_frame_budget(true), Some(3));
    fairness.charge_rx(3, true);
    assert!(fairness.network_turn_owed());
    assert_eq!(fairness.rx_protocol_frame_budget(true), Some(1));

    fairness.charge_tx(1, true);
    assert!(!fairness.network_turn_owed());
    assert_eq!(fairness.deficit(), 7);
}

#[test]
fn a_large_aggregate_keeps_its_rx_credit_while_rx_waits() {
    let mut fairness = RxTxFairness::new();
    fairness.charge_tx(32, true);
    assert_eq!(fairness.deficit(), -32);
    assert_eq!(fairness.rx_protocol_frame_budget(true), Some(40));
    // An empty transaction still repays one frame.
    fairness.charge_tx(0, true);
    assert_eq!(fairness.deficit(), -33);
}

#[test]
fn tx_admitted_while_no_rx_waits_leaves_no_credit() {
    let mut fairness = RxTxFairness::new();
    for _ in 0..1_000 {
        fairness.charge_tx(32, false);
    }
    assert_eq!(fairness.deficit(), 0);
    assert_eq!(
        fairness.rx_protocol_frame_budget(true),
        Some(RX_TX_FAIRNESS_QUANTUM_FRAMES as usize)
    );
}

#[test]
fn no_waiting_network_tx_leaves_the_rx_batch_to_the_role() {
    assert_eq!(RxTxFairness::new().rx_protocol_frame_budget(false), None);
}
