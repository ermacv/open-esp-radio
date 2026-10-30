use super::*;
use crate::claims::{AIR, Claim};

const A: &str = "board:02:00:00:00:00:0a";
const B: &str = "board:02:00:00:00:00:0b";
const NOW: u64 = 1_000_000;

fn stand() -> Vec<String> {
    vec![A.to_owned(), B.to_owned(), readiness::ROUTER.to_owned()]
}

fn queued(owner: &str, claims: Vec<Claim>) -> Queued {
    Queued {
        owner: owner.to_owned(),
        work: format!("work of {owner}"),
        scenarios: Vec::new(),
        run: None,
        claims,
        priority: Priority::Ordinary,
        estimate_secs: 60,
        client: ProcessIdentity {
            pid: 1,
            start_ticks: 1,
        },
    }
}

fn grant(scheduler: &mut Scheduler) -> Vec<u64> {
    let mut next = 0;
    scheduler
        .grant(NOW, &mut || {
            next += 1;
            format!("token-{next}")
        })
        .into_iter()
        .map(|granted| granted.lease)
        .collect()
}

/// A scheduler whose every resource was confirmed ready.
fn ready() -> Scheduler {
    let (mut scheduler, resumed) = Scheduler::resume(State::default(), stand(), NOW);
    for recovery in &resumed.recoveries {
        assert!(scheduler.confirm(recovery, Confirmation::Ready));
    }
    scheduler
}

fn recovering(scheduler: &Scheduler, resource: &str) -> Recovery {
    match scheduler.readiness(resource) {
        Some(Readiness::Recovering { generation, .. }) => Recovery {
            resource: resource.to_owned(),
            generation: *generation,
        },
        other => panic!("{resource} is not recovering: {other:?}"),
    }
}

#[test]
fn a_resource_never_confirmed_is_recovered_before_its_first_grant() {
    let (mut scheduler, resumed) = Scheduler::resume(State::default(), stand(), NOW);
    assert_eq!(resumed.recoveries.len(), 3);
    let id = scheduler.enqueue(queued("ann", vec![Claim::board(&A[6..])]), 1);
    assert!(grant(&mut scheduler).is_empty());
    let a = recovering(&scheduler, A);
    assert!(scheduler.confirm(&a, Confirmation::Ready));
    assert_eq!(grant(&mut scheduler), vec![id]);
    assert!(matches!(
        scheduler.readiness(A),
        Some(Readiness::InUse { lease, generation }) if *lease == id && *generation > a.generation
    ));
}

#[test]
fn the_end_of_a_lease_leaves_its_board_unavailable_until_its_driver_confirms() {
    let mut scheduler = ready();
    let first = scheduler.enqueue(queued("ann", vec![Claim::board(&A[6..])]), 1);
    assert_eq!(grant(&mut scheduler), vec![first]);
    let ended = scheduler
        .end(first, LeaseOutcome::Abandoned, NOW + 1000)
        .unwrap();
    assert_eq!(ended.record.outcome, LeaseOutcome::Abandoned);
    assert_eq!(ended.recoveries, vec![recovering(&scheduler, A)]);
    let second = scheduler.enqueue(queued("bob", vec![Claim::board(&A[6..])]), 2);
    assert!(grant(&mut scheduler).is_empty());
    assert!(scheduler.confirm(&ended.recoveries[0], Confirmation::Ready));
    assert_eq!(grant(&mut scheduler), vec![second]);
}

#[test]
fn a_confirmation_of_an_older_generation_changes_nothing() {
    let mut scheduler = ready();
    let first = scheduler.enqueue(queued("ann", vec![Claim::board(&A[6..])]), 1);
    grant(&mut scheduler);
    let old = scheduler
        .end(first, LeaseOutcome::Released, NOW)
        .unwrap()
        .recoveries
        .remove(0);
    assert!(scheduler.confirm(&old, Confirmation::Ready));
    let second = scheduler.enqueue(queued("bob", vec![Claim::board(&A[6..])]), 2);
    assert_eq!(grant(&mut scheduler), vec![second]);
    // The first lease's recovery reports late, during the second lease.
    assert!(!scheduler.confirm(&old, Confirmation::Ready));
    assert!(
        matches!(scheduler.readiness(A), Some(Readiness::InUse { lease, .. }) if *lease == second)
    );
    let current = scheduler
        .end(second, LeaseOutcome::Released, NOW)
        .unwrap()
        .recoveries
        .remove(0);
    assert!(!scheduler.confirm(&old, Confirmation::Ready));
    assert_eq!(recovering(&scheduler, A), current);
}

#[test]
fn conflicting_requests_wait_and_independent_ones_run_at_once() {
    let mut scheduler = ready();
    let first = scheduler.enqueue(queued("ann", vec![Claim::board(&A[6..])]), 1);
    let conflicting = scheduler.enqueue(queued("bob", vec![Claim::board(&A[6..])]), 2);
    let independent = scheduler.enqueue(queued("cat", vec![Claim::board(&B[6..])]), 3);
    assert_eq!(grant(&mut scheduler), vec![first, independent]);
    assert!(grant(&mut scheduler).is_empty());
    let a = scheduler
        .end(first, LeaseOutcome::Released, NOW)
        .unwrap()
        .recoveries
        .remove(0);
    scheduler.confirm(&a, Confirmation::Ready);
    assert_eq!(grant(&mut scheduler), vec![conflicting]);
}

#[test]
fn a_board_left_unconfirmed_closes_the_air_to_another_board() {
    let mut scheduler = ready();
    let first = scheduler.enqueue(
        queued("ann", vec![Claim::board(&A[6..]), Claim::shared(AIR)]),
        1,
    );
    grant(&mut scheduler);
    let a = scheduler
        .end(first, LeaseOutcome::Abandoned, NOW)
        .unwrap()
        .recoveries
        .remove(0);
    let radio = scheduler.enqueue(
        queued("bob", vec![Claim::board(&B[6..]), Claim::shared(AIR)]),
        2,
    );
    assert!(
        grant(&mut scheduler).is_empty(),
        "board A may still radiate"
    );
    assert!(scheduler.confirm(
        &a,
        Confirmation::Unavailable {
            reason: "neither its image nor its ROM answered".to_owned(),
            quiet: false,
        }
    ));
    assert!(
        grant(&mut scheduler).is_empty(),
        "a quarantine is not quiet"
    );
    let again = scheduler.release(A).unwrap();
    assert!(scheduler.confirm(
        &again,
        Confirmation::Unavailable {
            reason: "its hub port is off".to_owned(),
            quiet: true,
        }
    ));
    assert_eq!(grant(&mut scheduler), vec![radio]);
}

#[test]
fn work_without_the_air_runs_beside_a_board_in_recovery() {
    let mut scheduler = ready();
    let first = scheduler.enqueue(queued("ann", vec![Claim::board(&A[6..])]), 1);
    grant(&mut scheduler);
    scheduler.end(first, LeaseOutcome::Released, NOW).unwrap();
    let quiet = scheduler.enqueue(queued("bob", vec![Claim::board(&B[6..])]), 2);
    assert_eq!(grant(&mut scheduler), vec![quiet]);
}

#[test]
fn a_failed_recovery_holds_the_resource_until_a_person_returns_it() {
    let mut scheduler = ready();
    let first = scheduler.enqueue(queued("ann", vec![Claim::board(&A[6..])]), 1);
    grant(&mut scheduler);
    let a = scheduler
        .end(first, LeaseOutcome::Released, NOW)
        .unwrap()
        .recoveries
        .remove(0);
    let reason = "neither its image nor its ROM answered".to_owned();
    scheduler.confirm(
        &a,
        Confirmation::Unavailable {
            reason: reason.clone(),
            quiet: false,
        },
    );
    let waiting = scheduler.enqueue(queued("bob", vec![Claim::board(&A[6..])]), 2);
    assert!(grant(&mut scheduler).is_empty());
    assert!(matches!(
        scheduler.readiness(A),
        Some(Readiness::Unavailable { reason: held, .. }) if *held == reason
    ));
    assert!(
        scheduler.release(B).is_none(),
        "a ready board is not returned"
    );
    let again = scheduler.release(A).unwrap();
    assert!(again.generation > a.generation);
    assert!(grant(&mut scheduler).is_empty());
    scheduler.confirm(&again, Confirmation::Ready);
    assert_eq!(grant(&mut scheduler), vec![waiting]);
}

#[test]
fn the_whole_stand_waits_for_every_resource() {
    let mut scheduler = ready();
    let first = scheduler.enqueue(queued("ann", vec![Claim::board(&A[6..])]), 1);
    grant(&mut scheduler);
    let a = scheduler
        .end(first, LeaseOutcome::Released, NOW)
        .unwrap()
        .recoveries
        .remove(0);
    let whole = scheduler.enqueue(queued("bob", Vec::new()), 2);
    assert!(grant(&mut scheduler).is_empty());
    scheduler.confirm(&a, Confirmation::Ready);
    let granted = scheduler.grant(NOW, &mut || "token".to_owned());
    assert_eq!(granted.len(), 1);
    assert_eq!(granted[0].lease, whole);
    assert_eq!(granted[0].resources.len(), 3, "boards and router");
}

#[test]
fn a_restart_ends_held_leases_and_recovers_their_resources_first() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let mut scheduler = ready();
    let held = scheduler.enqueue(queued("ann", vec![Claim::board(&A[6..])]), 1);
    grant(&mut scheduler);
    scheduler.enqueue(queued("bob", vec![Claim::board(&B[6..])]), 2);
    scheduler.save(&path).unwrap();

    let (mut scheduler, resumed) = Scheduler::open(&path, stand(), NOW + 5000).unwrap();
    assert_eq!(resumed.abandoned.len(), 1);
    assert_eq!(resumed.abandoned[0].id, held);
    assert_eq!(resumed.abandoned[0].outcome, LeaseOutcome::Abandoned);
    assert_eq!(resumed.recoveries, vec![recovering(&scheduler, A)]);
    assert!(
        scheduler.state.queue.is_empty(),
        "waiters left with their clients"
    );
    let next = scheduler.enqueue(queued("bob", vec![Claim::board(&A[6..])]), 3);
    assert!(next > held, "lease identities are never reused");
    assert!(grant(&mut scheduler).is_empty());
    scheduler.confirm(&resumed.recoveries[0], Confirmation::Ready);
    assert_eq!(grant(&mut scheduler), vec![next]);
}

#[test]
fn a_restart_runs_an_unfinished_recovery_again_under_a_new_generation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let mut scheduler = ready();
    let first = scheduler.enqueue(queued("ann", vec![Claim::board(&A[6..])]), 1);
    grant(&mut scheduler);
    let before = scheduler
        .end(first, LeaseOutcome::Released, NOW)
        .unwrap()
        .recoveries
        .remove(0);
    scheduler.save(&path).unwrap();

    let (mut scheduler, resumed) = Scheduler::open(&path, stand(), NOW).unwrap();
    assert!(resumed.abandoned.is_empty());
    let after = resumed.recoveries[0].clone();
    assert_eq!(after.resource, A);
    assert!(after.generation > before.generation);
    assert!(!scheduler.confirm(&before, Confirmation::Ready));
    assert!(scheduler.confirm(&after, Confirmation::Ready));
}

#[test]
fn a_state_of_another_schema_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    std::fs::write(&path, br#"{"schema": 1}"#).unwrap();
    let error = Scheduler::open(&path, stand(), NOW).unwrap_err();
    assert!(error.to_string().contains("schema 1"), "{error}");
}

#[test]
fn a_board_under_maintenance_waits_for_every_other_owner() {
    let mut scheduler = ready();
    scheduler.set_maintenance(crate::Maintenance {
        mac: A[6..].to_owned(),
        owner: "ann".to_owned(),
        reason: "rework".to_owned(),
        since_unix: 1,
        unknown: Default::default(),
    });
    let other = scheduler.enqueue(queued("bob", vec![Claim::board(&A[6..])]), 1);
    let maintainer = scheduler.enqueue(queued("ann", vec![Claim::board(&A[6..])]), 2);
    assert_eq!(grant(&mut scheduler), vec![maintainer]);
    let a = scheduler
        .end(maintainer, LeaseOutcome::Released, NOW)
        .unwrap()
        .recoveries
        .remove(0);
    scheduler.confirm(&a, Confirmation::Ready);
    assert!(grant(&mut scheduler).is_empty());
    assert!(scheduler.clear_maintenance(&A[6..]));
    assert_eq!(grant(&mut scheduler), vec![other]);
}

#[test]
fn a_merged_owner_keeps_one_balance() {
    let mut scheduler = ready();
    let held = scheduler.enqueue(queued("old", vec![Claim::board(&A[6..])]), 1);
    grant(&mut scheduler);
    scheduler.end(held, LeaseOutcome::Released, NOW + 60_000);
    scheduler.merge_owner("old", "new");
    assert!(!scheduler.forget_owner("old"));
    assert!(scheduler.forget_owner("new"));
}
