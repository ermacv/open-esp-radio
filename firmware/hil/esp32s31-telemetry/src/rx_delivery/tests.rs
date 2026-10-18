use super::*;

#[test]
fn sequence_tracker_separates_gap_recovery_duplicate_and_terminal_tail() {
    let mut tracker = SequenceTracker::default();
    for sequence in [0, 2, 1, 1, -1, 3] {
        tracker.observe(sequence);
    }
    assert_eq!(tracker.evidence.data_units, 5);
    assert_eq!(tracker.evidence.gap_events, 1);
    assert_eq!(tracker.evidence.forward_missing, 1);
    assert_eq!(tracker.evidence.late_recovered, 1);
    assert_eq!(tracker.evidence.duplicates, 1);
    assert_eq!(tracker.evidence.control_markers, 1);
    assert_eq!(tracker.evidence.data_after_terminal, 1);
}

#[test]
fn ledger_reports_the_first_exact_enqueue_consumer_divergence() {
    let mut ledger = SequenceLedger::<8>::default();
    for sequence in [10, 11, 12] {
        ledger.push(sequence);
    }
    ledger.consume(10);
    ledger.consume(12);
    ledger.consume(11);
    let evidence = ledger.finish();
    assert_eq!(evidence.matched, 2);
    assert_eq!(evidence.skipped_before_observed, 1);
    assert_eq!(evidence.unexpected_consumer, 1);
    assert_eq!(evidence.first_expected, Some(11));
    assert_eq!(evidence.first_observed, Some(12));
}

#[test]
fn session_tracker_accounts_network_drop_without_entering_ledger() {
    let mut tracker = RxDeliveryTracker::<8>::new();
    tracker.begin(7);
    tracker.admitted(0, Some((0, 100)));
    tracker.dropped(1, Some((0, 101)), NetworkDropReason::QueueFull);
    tracker.consumed(7, 0);
    tracker.consumed(8, 1);
    let evidence = tracker
        .finish(7, RxReorderDeliveryEvidence::default())
        .unwrap();
    assert_eq!(evidence.post_reorder.data_units, 2);
    assert_eq!(evidence.network_enqueued.data_units, 1);
    assert_eq!(evidence.udp_consumer.data_units, 1);
    assert_eq!(evidence.network_queue_full, 1);
    assert_eq!(evidence.consumer_ledger.matched, 1);
}

#[test]
fn session_preserves_drop_causes_and_resets_them_between_runs() {
    let mut tracker = RxDeliveryTracker::<8>::new();
    // Events outside a session must not leak into the next run.
    tracker.dropped(0, None, NetworkDropReason::PoolExhausted);
    tracker.begin(7);
    tracker.admitted(0, None);
    for (sequence, reason) in [
        (1, NetworkDropReason::QueueFull),
        (2, NetworkDropReason::InvalidLength),
        (3, NetworkDropReason::PoolExhausted),
        (4, NetworkDropReason::PoolExhausted),
        (5, NetworkDropReason::LinkDown),
    ] {
        tracker.dropped(sequence, None, reason);
    }
    tracker.consumed(7, 0);
    let evidence = tracker
        .finish(7, RxReorderDeliveryEvidence::default())
        .unwrap();
    assert_eq!(evidence.post_reorder.data_units, 6);
    assert_eq!(evidence.network_enqueued.data_units, 1);
    assert_eq!(evidence.udp_consumer.data_units, 1);
    assert_eq!(evidence.network_queue_full, 1);
    assert_eq!(evidence.network_invalid_length, 1);
    assert_eq!(evidence.network_pool_exhausted, 2);
    assert_eq!(evidence.network_link_down, 1);
    assert_eq!(evidence.consumer_ledger.matched, 1);
    assert_eq!(evidence.consumer_ledger.enqueued_not_consumed, 0);

    tracker.begin(8);
    let next = tracker
        .finish(8, RxReorderDeliveryEvidence::default())
        .unwrap();
    assert_eq!(next, RxDeliveryEvidence::default());
}

#[test]
fn forward_gap_retains_udp_and_mac_identity_including_mac_wrap() {
    let mut tracker = RxDeliveryTracker::<8>::new();
    tracker.begin(1);
    tracker.admitted(7, Some((0, 4095)));
    tracker.admitted(65, Some((0, 0)));
    let gap = tracker.mac.evidence.first_forward_gap.unwrap();
    assert_eq!((gap.previous_udp, gap.current_udp), (7, 65));
    assert_eq!((gap.tid, gap.previous_mac, gap.current_mac), (0, 4095, 0));
    tracker.admitted(100, Some((0, 9)));
    assert_eq!(tracker.mac.evidence.first_forward_gap, Some(gap));
    assert_eq!(tracker.mac.correlated_gaps, 2);
    assert_eq!(tracker.mac.gap_samples[0], Some(gap));
    assert_eq!(tracker.mac.gap_samples[1].unwrap().current_udp, 100);
    tracker.begin(2);
    assert_eq!(tracker.mac.evidence.first_forward_gap, None);
    assert_eq!(tracker.mac.correlated_gaps, 0);
    assert!(tracker.mac.gap_samples.iter().all(Option::is_none));
}
#[test]
fn forward_gap_does_not_invent_identity_across_tid_or_missing_metadata() {
    for mac in [None, Some((1, 15)), Some((0, 4096))] {
        let mut tracker = RxDeliveryTracker::<8>::new();
        tracker.begin(1);
        tracker.admitted(7, Some((0, 14)));
        tracker.admitted(65, mac);
        assert_eq!(tracker.mac.evidence.first_forward_gap, None);
        assert_eq!(tracker.mac.correlated_gaps, 0);
    }
}

#[test]
fn full_gap_recorder_preserves_delivery_and_exposes_truncation() {
    let mut tracker = RxDeliveryTracker::<4>::new();
    tracker.begin(1);
    assert!(tracker.completed_gap_samples(1).is_none());
    let count = FORWARD_GAP_SAMPLE_CAPACITY as u32 + 4;
    for index in 0..=count {
        tracker.admitted((index * 2) as i32, Some((0, index as u16)));
        tracker.consumed(1, (index * 2) as i32);
    }
    let evidence = tracker
        .finish(1, RxReorderDeliveryEvidence::default())
        .unwrap();
    assert_eq!(tracker.completed_gap_samples(1).unwrap().0, count);
    assert!(tracker.completed_gap_samples(2).is_none());
    assert_eq!(evidence.consumer_ledger.matched, count + 1);
    assert_eq!(evidence.consumer_ledger.overflow, 0);
    assert_eq!(evidence.post_reorder.gap_events, count);
    assert_eq!(tracker.mac.correlated_gaps, count);
    for (index, gap) in tracker.mac.gap_samples.iter().enumerate() {
        let gap = gap.unwrap();
        assert_eq!(gap.previous_udp, index as u32 * 2);
        assert_eq!(gap.current_udp, (index as u32 + 1) * 2);
        assert_eq!(gap.current_mac, gap.previous_mac + 1);
    }
    tracker.admitted(1000, Some((0, 1000)));
    assert_eq!(tracker.mac.correlated_gaps, count);
    tracker.begin(2);
    assert!(tracker.completed_gap_samples(1).is_none());
    assert!(tracker.completed_gap_samples(2).is_none());
}
