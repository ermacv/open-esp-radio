use super::MacTxStatisticsSnapshot;

#[test]
fn tx_statistics_delta_wraps_at_the_register_width() {
    let earlier = MacTxStatisticsSnapshot {
        tx_rts: u32::MAX,
        tx_cts: 9,
        track: 20,
        trcts: 30,
    };
    let later = MacTxStatisticsSnapshot {
        tx_rts: 2,
        tx_cts: 12,
        track: 25,
        trcts: 37,
    };

    assert_eq!(
        later.wrapping_delta_since(earlier),
        MacTxStatisticsSnapshot {
            tx_rts: 3,
            tx_cts: 3,
            track: 5,
            trcts: 7,
        }
    );
}
