use std::{process::Command, time::Duration};

use super::*;
use crate::{
    LeaseOutcome,
    state::{State, Ticket},
};
use oer_stand_claims::Claim;

fn holder(arbiter: &Arbiter, id: u64, owner: &str, process: ProcessIdentity, granted_unix: u64) {
    arbiter
        .transaction(|state| {
            state.holders.push(Holder {
                ticket: ticket(id, owner, process),
                token: String::from("other"),
                granted_unix,
                reason: None,
                preempted: None,
                unknown: Default::default(),
            });
            Ok(())
        })
        .unwrap();
}

fn ticket(id: u64, owner: &str, process: ProcessIdentity) -> Ticket {
    Ticket {
        id,
        owner: owner.into(),
        work: format!("work {id}"),
        estimate_secs: 60,
        process,
        enqueued_unix: oer_durable::unix_seconds(),
        claims: vec![Claim::board("AA")],
        priority: Default::default(),
        job: None,
        run: None,
        unknown: Default::default(),
    }
}

fn state(arbiter: &Arbiter) -> State {
    arbiter.transaction(|state| Ok(state.clone())).unwrap()
}

/// A holder process that its own thread reaps once a signal ends it.
fn holder_process() -> ProcessIdentity {
    let mut child = Command::new("sleep").arg("60").spawn().unwrap();
    let identity = ProcessIdentity::of(child.id()).unwrap();
    std::thread::spawn(move || child.wait());
    identity
}

#[test]
fn a_preempted_holder_is_terminated_and_recorded_with_who_and_why() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let process = holder_process();
    holder(&arbiter, 7, "phy", process, oer_durable::unix_seconds());
    let end = arbiter
        .preempt(7, "infra", "stuck calibration", Duration::from_secs(10))
        .unwrap();
    assert_eq!(end, PreemptEnd::Released);
    assert!(!process.alive());
    assert!(state(&arbiter).holders.is_empty());
    let record = arbiter.history().unwrap().pop().unwrap();
    assert_eq!(record.outcome, LeaseOutcome::PreemptedOnRequest);
    let preemption = record.preempted.unwrap();
    assert_eq!(
        (preemption.by.as_str(), preemption.reason.as_str()),
        ("infra", "stuck calibration")
    );
}

#[test]
fn a_preempted_lease_is_charged_only_up_to_its_preemption() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let (process, _) = (ProcessIdentity::current().unwrap(), ());
    // Granted ten minutes ago; preempting this test process itself is not
    // signalled here, only marked.
    let granted = oer_durable::unix_seconds() - 600;
    holder(&arbiter, 3, "phy", process, granted);
    arbiter.mark_preempted(3, "infra", "why").unwrap();
    let before = crate::balance::of(&state(&arbiter).balances, "phy");
    std::thread::sleep(Duration::from_millis(1100));
    let after = crate::balance::of(&state(&arbiter).balances, "phy");
    assert_eq!(before, after, "no charge after the preemption");
    let holder = state(&arbiter).holders.pop().unwrap();
    let charged = charged_ms(&holder, 3_600_000);
    assert!((599_000..=602_000).contains(&charged), "{charged}");
}

#[test]
fn only_a_held_lease_can_be_preempted_and_only_with_a_reason() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let process = ProcessIdentity::current().unwrap();
    arbiter
        .transaction(|state| {
            state.queue.push(ticket(4, "phy", process));
            Ok(())
        })
        .unwrap();
    let error = arbiter
        .preempt(4, "infra", "why", Duration::ZERO)
        .unwrap_err()
        .to_string();
    assert!(error.contains("waiting, not held"), "{error}");
    assert!(
        arbiter
            .preempt(9, "infra", "why", Duration::ZERO)
            .unwrap_err()
            .to_string()
            .contains("no lease #9")
    );
    assert!(arbiter.preempt(4, "infra", " ", Duration::ZERO).is_err());
}
