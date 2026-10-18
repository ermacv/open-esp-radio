use super::*;

#[test]
fn standby_timing_requires_exact_owner_balance_across_snapshots() {
    let tx = TxRadioEvidence::default();
    let mut timing = TxAggregateTimingEvidence {
        standby_prepared: 1,
        standby_pending_end: 1,
        ..Default::default()
    };
    assert!(validate_tx_timing(tx, timing).is_ok());
    timing.standby_pending_end = 0;
    assert!(validate_tx_timing(tx, timing).is_err());
    timing.standby_prepared = 0;
    timing.standby_pending_start = 1;
    timing.standby_cancelled = 1;
    assert!(validate_tx_timing(tx, timing).is_ok());
    timing.standby_cancelled = 0;
    timing.standby_published = 1;
    assert!(validate_tx_timing(tx, timing).is_ok());
    timing.standby_pending_start = 0;
    assert!(validate_tx_timing(tx, timing).is_err());
}
