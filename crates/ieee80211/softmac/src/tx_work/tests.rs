use super::*;

#[test]
fn retry_accounts_for_its_own_length_and_rate() {
    let mut work = MacTxWork::default();
    work.record(3_000, 3, NonZeroU32::new(150_000));
    work.record(1_000, 1, NonZeroU32::new(65_000));
    assert_eq!(work.publications, 2);
    assert_eq!(work.psdu_bytes, 4_000);
    assert_eq!(work.mpdus, 4);
    assert_eq!(work.nominal_data_micros, 160 + 124);
    assert_eq!(work.unestimated_publications, 0);
    assert!(!work.saturated);
}

#[test]
fn unknown_rate_preserves_work_without_inventing_time() {
    let mut work = MacTxWork::default();
    work.record(1_000, 1, None);
    assert_eq!(work.publications, 1);
    assert_eq!(work.psdu_bytes, 1_000);
    assert_eq!(work.mpdus, 1);
    assert_eq!(work.nominal_data_micros, 0);
    assert_eq!(work.unestimated_publications, 1);
}

#[test]
fn overflow_is_explicit_and_never_wraps_into_a_smaller_charge() {
    let mut work = MacTxWork {
        psdu_bytes: u32::MAX - 1,
        ..MacTxWork::default()
    };
    work.record(2, 1, NonZeroU32::new(1));
    assert_eq!(work.psdu_bytes, u32::MAX);
    assert!(work.saturated);
    assert_eq!(work.nominal_data_micros, 16_000);
}

#[test]
fn contention_retains_each_retry_selection_and_marks_missing_facts() {
    use crate::tx_cost::TxContention;
    let mut work = MacTxWork::default();
    work.record_publication(
        1000,
        1,
        None,
        None,
        Some(TxContention {
            aifsn: 3,
            backoff_slots: 0,
        }),
    );
    work.record_publication(
        1000,
        1,
        None,
        None,
        Some(TxContention {
            aifsn: 7,
            backoff_slots: 31,
        }),
    );
    work.record(1000, 1, None);
    assert_eq!(work.publications, 3);
    assert_eq!(work.aifs_slots, 10);
    assert_eq!(work.backoff_slots, 31);
    assert_eq!(work.unreported_contention, 1);
    assert!(!work.saturated);
}

#[test]
fn contention_overflow_remains_a_lower_bound() {
    use crate::tx_cost::TxContention;
    let mut work = MacTxWork {
        aifs_slots: u32::MAX,
        backoff_slots: u32::MAX,
        ..MacTxWork::default()
    };
    work.record_publication(
        100,
        1,
        None,
        None,
        Some(TxContention {
            aifsn: 1,
            backoff_slots: 1,
        }),
    );
    assert_eq!(work.aifs_slots, u32::MAX);
    assert_eq!(work.backoff_slots, u32::MAX);
    assert!(work.saturated);
}

#[test]
fn scheduling_charge_includes_each_publications_explicit_overhead() {
    let work = MacTxWork {
        publications: 3,
        ppdu_micros: 400,
        ..MacTxWork::default()
    };
    assert_eq!(work.estimated_exchange_micros(48).unwrap().get(), 544);
}

#[test]
fn incomplete_or_overflowed_receipt_cannot_be_settled_as_free_service() {
    assert_eq!(MacTxWork::new().estimated_exchange_micros(0), None);
    for work in [
        MacTxWork {
            publications: 1,
            ppdu_micros: 100,
            unestimated_ppdus: 1,
            ..MacTxWork::default()
        },
        MacTxWork {
            publications: 1,
            ppdu_micros: 100,
            saturated: true,
            ..MacTxWork::default()
        },
        MacTxWork {
            publications: u32::MAX,
            ppdu_micros: 100,
            ..MacTxWork::default()
        },
        MacTxWork {
            publications: 1,
            ppdu_micros: u32::MAX,
            ..MacTxWork::default()
        },
    ] {
        assert_eq!(work.estimated_exchange_micros(48), None);
    }
}

#[test]
fn absorbed_work_adds_every_counter_and_keeps_saturation() {
    let mut aggregate = MacTxWork::new();
    aggregate.record(1_600, 2, NonZeroU32::new(65_000));
    let mut retry = MacTxWork::new();
    retry.record(800, 1, None);
    aggregate.absorb(retry);
    assert_eq!(aggregate.publications, 2);
    assert_eq!(aggregate.psdu_bytes, 2_400);
    assert_eq!(aggregate.mpdus, 3);
    assert_eq!(aggregate.unestimated_publications, 1);
    assert_eq!(aggregate.unestimated_ppdus, 2);
    assert_eq!(aggregate.unreported_contention, 2);
    assert!(!aggregate.saturated);

    let mut full = MacTxWork {
        publications: u32::MAX,
        ..MacTxWork::new()
    };
    full.absorb(retry);
    assert_eq!(full.publications, u32::MAX);
    assert!(full.saturated);
}
