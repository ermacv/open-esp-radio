use super::{TxPerformanceCounters, TxPerformanceSample};

const fn sample(cycles: u32, instructions: u32) -> TxPerformanceSample {
    TxPerformanceSample {
        cycles,
        instructions,
    }
}

#[test]
fn promotion_accounting_separates_copy_from_owner_transitions() {
    let counters = TxPerformanceCounters::new();
    counters.record_promotion(
        1_536,
        sample(100, 200),
        sample(110, 205),
        sample(117, 209),
        sample(20, 10),
        sample(120, 210),
        sample(150, 225),
        sample(170, 235),
        sample(180, 240),
    );
    counters.record_promotion_no_credit(sample(1_000, 2_000), sample(1_012, 2_007));

    let snapshot = counters.snapshot();
    assert_eq!(snapshot.promotion_attempts, 2);
    assert_eq!(snapshot.promotion_successes, 1);
    assert_eq!(snapshot.promotion_no_credit, 1);
    assert_eq!(snapshot.promotion_bytes, 1_536);
    assert_eq!(snapshot.promotion_cycles, 92);
    assert_eq!(snapshot.promotion_instructions, 47);
    assert_eq!(snapshot.promotion_copy_cycles, 20);
    assert_eq!(snapshot.promotion_publication_cycles, 10);
    assert_eq!(snapshot.promotion_source_release_cycles, 20);
    assert_eq!(snapshot.promotion_radio_claim_cycles, 10);
    assert_eq!(snapshot.promotion_unattributed_cycles(), 3);
    assert_eq!(snapshot.promotion_unattributed_instructions(), 1);
}
