use super::*;

#[test]
fn stable_ht_observation_keeps_valid_mcs32_separate_from_width_mismatch() {
    let valid = HtRxObservation::from(HtSignal {
        mcs: 32,
        channel_width_mhz: 40,
        aggregation: true,
        short_guard_interval: false,
    });
    assert!(valid.duplicate_mcs32);
    assert!(!valid.duplicate_mcs32_width_mismatch);

    let mismatch = HtRxObservation::from(HtSignal {
        mcs: 32,
        channel_width_mhz: 20,
        aggregation: false,
        short_guard_interval: false,
    });
    assert!(!mismatch.duplicate_mcs32);
    assert!(mismatch.duplicate_mcs32_width_mismatch);

    let ordinary = HtRxObservation::from(HtSignal {
        mcs: 7,
        channel_width_mhz: 40,
        aggregation: false,
        short_guard_interval: true,
    });
    assert!(!ordinary.duplicate_mcs32);
    assert!(!ordinary.duplicate_mcs32_width_mismatch);
}
