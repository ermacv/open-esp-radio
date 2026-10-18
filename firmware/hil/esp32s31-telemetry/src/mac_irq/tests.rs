use super::*;

#[test]
fn classification_separates_rx_tx_and_extra_drain_work() {
    let counters = MacIrqClassificationCounters::new();
    counters.record(true, EVENT_RX_SUCCESS, false, false);
    counters.record(true, EVENT_TX_COMPLETE, false, false);
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.rx_only_entries, 1);
    assert_eq!(snapshot.tx_only_entries, 1);
    assert_eq!(snapshot.extra_nonzero_snapshots, 0);
}
