use super::*;

fn event(
    action: AirtimeAction,
    grant: u32,
    charge: u32,
    outstanding: u32,
) -> AirtimeObservation<AccessPointAirtimePeer> {
    AirtimeObservation {
        key: AccessPointAirtimePeer::Group,
        action,
        grant_micros: grant,
        charged_micros: charge,
        balance_after_event_micros: -250,
        outstanding,
        outstanding_micros: u64::from(outstanding) * 1000,
    }
}

#[test]
fn reservations_reconcile_and_cancellations_do_not_count_as_service() {
    let mut history = AirtimeHistory::new();
    history.observe(event(AirtimeAction::Granted, 1000, 0, 1));
    history.observe(event(AirtimeAction::Granted, 1000, 0, 2));
    history.observe(event(AirtimeAction::Settled, 1000, 1250, 1));
    history.observe(event(AirtimeAction::Cancelled, 1000, 0, 0));
    let p = history.peers[0].unwrap();
    assert_eq!(
        p.grants,
        p.settlements + p.cancellations + u64::from(p.outstanding)
    );
    assert_eq!(
        p.granted_micros,
        p.settled_grants_micros + p.cancelled_grants_micros + p.outstanding_micros
    );
    assert_eq!(p.charged_micros, 1250);
    assert_eq!(p.maximum_outstanding, 2);
    assert_eq!(history.report.peer_records, 1);
    assert!(!history.report.saturated);
}

#[test]
fn full_history_does_not_evict_old_association_evidence() {
    let mut history = AirtimeHistory::new();
    history.observe(event(AirtimeAction::Granted, 1000, 0, 1));
    let mut entry = history.peers[0].unwrap();
    for (index, slot) in history.peers.iter_mut().enumerate() {
        entry.peer = WifiAirtimePeer::Unicast {
            address: [2, 0, 0, 0, 0, 1],
            association_id: 1,
            association_epoch: index as u32 + 1,
        };
        *slot = Some(entry);
    }
    history.report.peer_records = 8;
    let before = history.peers;
    history.observe(event(AirtimeAction::Granted, 1000, 0, 1));
    assert_eq!(history.peers, before);
    assert_eq!(history.report.dropped_events, 1);
}

#[test]
fn overflow_marks_the_report_incomplete_instead_of_wrapping() {
    let mut history = AirtimeHistory::new();
    history.observe(event(AirtimeAction::Granted, 1000, 0, 1));
    history.peers[0].as_mut().unwrap().granted_micros = u64::MAX;
    history.observe(event(AirtimeAction::Granted, 1, 0, 2));
    assert!(history.report.saturated);
    assert_eq!(history.peers[0].unwrap().granted_micros, u64::MAX);
}
