use super::*;

#[test]
fn ampdu_status_joins_block_ack_and_individual_retries() {
    let status = MacAmpduTxStatus {
        result: MacAmpduTxResult::Delivered,
        original_subframes: 3,
        aggregate_attempts: 2,
        aggregate_rate: 7_u8,
        block_acknowledged_subframes: 2,
        individual_retries: MacIndividualRetries::NONE,
        ack_snr_db: None,
    };
    assert_eq!(status.delivered_subframes(), 2);
    assert!(!status.fully_delivered());

    let mut delivered = MacAmpduTxStatus {
        original_subframes: 4,
        ..status
    };
    for attempts in [2, 1] {
        delivered.individual_retries.record(MacTxStatus {
            result: MacTxResult::Transmitted,
            attempts,
            final_rate: 5,
            acknowledged: Some(true),
            ack_snr_db: Some(12),
            airtime_micros: None,
        });
    }
    assert_eq!(delivered.delivered_subframes(), 4);
    assert_eq!(delivered.total_publication_attempts(), 5);
    assert_eq!(delivered.individual_retries.final_rate, Some(5));
    assert!(delivered.fully_delivered());
}

