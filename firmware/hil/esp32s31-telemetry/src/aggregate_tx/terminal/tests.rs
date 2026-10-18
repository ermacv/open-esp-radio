use super::*;
use oer_ieee80211_softmac::{MacIndividualRetries, MacTxResult, MacTxStatus};

fn status(result: MacAmpduTxResult, acknowledged: u16) -> MacAmpduTxStatus<()> {
    MacAmpduTxStatus {
        result,
        original_subframes: 3,
        aggregate_attempts: 2,
        aggregate_rate: (),
        block_acknowledged_subframes: acknowledged,
        individual_retries: MacIndividualRetries::NONE,
        ack_snr_db: None,
    }
}

#[test]
fn terminal_accounting_separates_recovered_and_exhausted_mpdus() {
    let counters = Counters::new();
    let before = counters.snapshot();
    counters.record(status(MacAmpduTxResult::Delivered, 3));
    counters.record(status(MacAmpduTxResult::Incomplete, 1));
    for recovered in [true, false] {
        let mut value = status(
            if recovered {
                MacAmpduTxResult::Delivered
            } else {
                MacAmpduTxResult::Incomplete
            },
            2,
        );
        value.individual_retries.record(MacTxStatus {
            result: if recovered {
                MacTxResult::Transmitted
            } else {
                MacTxResult::HardwareTimeout
            },
            attempts: 2,
            final_rate: (),
            acknowledged: Some(recovered),
            ack_snr_db: None,
            airtime_micros: None,
        });
        counters.record(value);
    }
    let after = delta(counters.snapshot(), before);
    assert_eq!(after.exchanges, 4);
    assert_eq!(after.mpdus, 12);
    assert_eq!(after.acknowledged, 9);
    assert_eq!(after.unacknowledged, 3);
    assert_eq!(after.ordinary_recovered, 1);
    assert_eq!(after.ordinary_failed, 1);
    assert_eq!(after.invalid_statuses, 0);
    assert_eq!(
        delta(counters.snapshot(), counters.snapshot()),
        StationTxTerminalEvidence::default()
    );
}

#[test]
fn malformed_terminal_status_does_not_invent_delivery() {
    let counters = Counters::new();
    counters.record(status(MacAmpduTxResult::Delivered, 4));
    counters.record(status(MacAmpduTxResult::Delivered, 2));
    counters.record(status(MacAmpduTxResult::Incomplete, 3));
    let value = counters.snapshot();
    assert_eq!(value.invalid_statuses, 3);
    assert_eq!(value.mpdus, 0);
    assert_eq!(value.exchanges, 0);
}
