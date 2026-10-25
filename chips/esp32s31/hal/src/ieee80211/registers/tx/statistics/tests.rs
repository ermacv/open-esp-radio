use super::MacTxStatisticsSnapshot;
use crate::ieee80211::registers::HostWords;

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

#[test]
fn the_snapshot_reads_the_four_counters_without_writing() {
    let before = HostWords::filled(0x1234_5678);
    let mut memory = before.clone();
    let snapshot = memory.owners().registers.tx_statistics_snapshot();
    assert_eq!(
        snapshot,
        MacTxStatisticsSnapshot {
            tx_rts: 0x1234_5678,
            tx_cts: 0x1234_5678,
            track: 0x1234_5678,
            trcts: 0x1234_5678,
        }
    );
    assert!(before.changes(&memory).is_empty());
}
