use std::{
    process::{Child, Command},
    time::Duration,
};

use super::*;
use oer_stand_claims::{AIR, Claim};

use oer_stand_journal::BoardEventKind;

use crate::{balance::Balance, state::State};

/// Boards by MAC, as a lease names them: locally administered, so no real
/// board's device lock is taken. The child that holds a supervised lease
/// has its own, so it never waits for a test of this process.
const CHIP_A: &str = "02:00:00:00:31:0A";
const CHIP_B: &str = "02:00:00:00:31:0B";
const HUNG: &str = "02:00:00:00:31:0C";

/// An arbiter in `directory` whose stand file has no boards: a whole-stand
/// lease locks the boards of the stand file, so it needs one to load.
fn arbiter(directory: &std::path::Path) -> Arbiter {
    let arbiter = Arbiter::at(directory).unwrap();
    let stand = arbiter.stand_file();
    std::fs::write(
        stand,
        "schema = 1\n[stand]\nid = \"test\"\nair = \"exclusive\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(stand, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    arbiter
}

fn request(owner: &str, work: &str) -> Request {
    Request {
        owner: owner.into(),
        work: work.into(),
        scenarios: Vec::new(),
        run: None,
        claims: Vec::new(),
    }
}

fn on_board(board: &str, request: Request) -> Request {
    Request {
        claims: vec![Claim::board(board), Claim::shared(AIR)],
        ..request
    }
}

/// A live process that stands for another requester.
fn other_process() -> (Child, ProcessIdentity) {
    let child = Command::new("sleep").arg("60").spawn().unwrap();
    let identity = ProcessIdentity::of(child.id()).unwrap();
    (child, identity)
}

fn stop(mut child: Child) {
    child.kill().unwrap();
    child.wait().unwrap();
}

fn ticket(id: u64, owner: &str, process: ProcessIdentity, claims: Vec<Claim>) -> Ticket {
    Ticket {
        id,
        owner: owner.into(),
        work: format!("work {id}"),
        estimate_secs: 60,
        process,
        enqueued_unix: oer_durable::unix_seconds(),
        claims: normalize(&claims),
        priority: Default::default(),
        job: None,
        run: None,
        unknown: Default::default(),
    }
}

fn hold(arbiter: &Arbiter, ticket: Ticket) {
    arbiter
        .transaction(|state| {
            state.holders.push(Holder {
                ticket,
                token: "other".into(),
                granted_unix: oer_durable::unix_seconds(),
                reason: None,
                preempted: None,
                unknown: Default::default(),
            });
            Ok(())
        })
        .unwrap();
}

fn queue(arbiter: &Arbiter, ticket: Ticket) {
    arbiter
        .transaction(|state| {
            state.next_id = state.next_id.max(ticket.id + 1);
            state.queue.push(ticket);
            Ok(())
        })
        .unwrap();
}

fn set_balance(arbiter: &Arbiter, owner: &str, minutes: i64) {
    arbiter
        .transaction(|state| {
            state.balances.insert(
                owner.into(),
                Balance {
                    balance_ms: minutes * 60_000,
                    last_active_unix_ms: oer_durable::unix_millis(),
                    ..Balance::default()
                },
            );
            Ok(())
        })
        .unwrap();
}

fn state(arbiter: &Arbiter) -> State {
    arbiter.transaction(|state| Ok(state.clone())).unwrap()
}

#[test]
fn a_free_stand_is_granted_and_released_into_history() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = arbiter(directory.path());
    let grant = arbiter
        .acquire_within(&request("wifi", "run a"), None)
        .unwrap();
    assert!(!grant.is_nested());
    let token = grant.context().unwrap().get(LEASE_KEY).unwrap().to_owned();
    assert_eq!(state(&arbiter).holders[0].token, token);
    assert_eq!(state(&arbiter).holders[0].ticket.claims, [Claim::stand()]);
    // The holding process and commands carrying its token join the lease.
    assert!(
        arbiter
            .acquire_within(&request("wifi", "inner"), None)
            .unwrap()
            .is_nested()
    );
    assert!(
        arbiter
            .acquire_within(&request("wifi", "inner"), Some(token.clone()))
            .unwrap()
            .is_nested()
    );
    drop(grant);
    assert!(state(&arbiter).holders.is_empty());
    let history = arbiter.history().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].outcome, LeaseOutcome::Released);
    assert_eq!(history[0].work, "run a");
    // A token of a finished lease is refused rather than silently queued.
    assert!(
        arbiter
            .acquire_within(&request("wifi", "late"), Some(token))
            .is_err()
    );
}

#[test]
fn a_run_is_named_by_its_holder_and_its_history_record() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = arbiter(directory.path());
    let grant = arbiter
        .acquire_within(
            &Request {
                run: Some(String::from("1000-a")),
                ..request("wifi", "run a")
            },
            None,
        )
        .unwrap();
    assert_eq!(
        arbiter.status().unwrap().holders[0].run.as_deref(),
        Some("1000-a")
    );
    drop(grant);
    assert_eq!(arbiter.history().unwrap()[0].run.as_deref(), Some("1000-a"));
}

#[test]
fn a_nested_command_may_use_only_what_the_enclosing_lease_holds() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = arbiter(directory.path());
    let grant = arbiter
        .acquire_within(&on_board(CHIP_A, request("phy", "series")), None)
        .unwrap();
    let token = grant.context().unwrap().get(LEASE_KEY).unwrap().to_owned();
    let inside = on_board(CHIP_A, request("phy", "run a"));
    assert!(
        arbiter
            .acquire_within(&inside, Some(token.clone()))
            .unwrap()
            .is_nested()
    );
    let other_board = on_board(CHIP_B, request("phy", "run b"));
    let error = arbiter
        .acquire_within(&other_board, Some(token))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains(&format!("board:{CHIP_B}")), "{error}");
}

#[test]
fn leases_on_different_boards_are_held_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = arbiter(directory.path());
    let (child, identity) = other_process();
    hold(
        &arbiter,
        ticket(
            7,
            "wifi",
            identity,
            vec![Claim::board(CHIP_A), Claim::shared(AIR)],
        ),
    );
    let grant = arbiter
        .acquire_within(&on_board(CHIP_B, request("802154", "peer")), None)
        .unwrap();
    assert_eq!(state(&arbiter).holders.len(), 2);
    drop(grant);
    // Exclusive air waits for every radio user.
    let rf = Request {
        claims: vec![Claim::board(CHIP_B), Claim::exclusive(AIR)],
        ..request("phy", "rf")
    };
    let waiter = {
        let arbiter = arbiter.clone();
        std::thread::spawn(move || {
            arbiter
                .acquire_within(&rf, None)
                .map(|grant| grant.is_nested())
        })
    };
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(state(&arbiter).queue.len(), 1);
    stop(child);
    assert!(!waiter.join().unwrap().unwrap());
}

#[test]
fn a_waiter_is_granted_when_the_holder_dies() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = arbiter(directory.path());
    let (holder, identity) = other_process();
    hold(&arbiter, ticket(7, "phy", identity, vec![]));
    let waiter = {
        let arbiter = arbiter.clone();
        std::thread::spawn(move || {
            let grant = arbiter.acquire_within(&request("bluetooth", "run b"), None);
            grant.map(|grant| grant.is_nested())
        })
    };
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(state(&arbiter).queue.len(), 1, "the request waits");
    stop(holder);
    assert!(!waiter.join().unwrap().unwrap());
    let history = arbiter.history().unwrap();
    assert_eq!(history[0].id, 7);
    assert_eq!(history[0].outcome, LeaseOutcome::Abandoned);
    assert_eq!(history[1].owner, "bluetooth");
}

#[test]
fn waiting_follows_arrival_order_and_dead_waiters_are_reaped() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = arbiter(directory.path());
    let (head, head_identity) = other_process();
    let (dead, dead_identity) = other_process();
    queue(&arbiter, ticket(1, "head", head_identity, vec![]));
    queue(&arbiter, ticket(2, "dead", dead_identity, vec![]));
    stop(dead);
    let waiter = {
        let arbiter = arbiter.clone();
        std::thread::spawn(move || {
            arbiter
                .acquire_within(&request("bluetooth", "run"), None)
                .is_ok()
        })
    };
    std::thread::sleep(Duration::from_millis(700));
    let waiting = state(&arbiter);
    assert_eq!(
        waiting.queue.iter().map(|t| t.id).collect::<Vec<_>>(),
        [1, 3],
        "the free stand stays reserved for the live head"
    );
    assert!(waiting.holders.is_empty());
    stop(head);
    assert!(waiter.join().unwrap());
}

#[test]
fn a_waiter_with_a_higher_balance_is_served_before_an_earlier_one() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = arbiter(directory.path());
    let (head, head_identity) = other_process();
    queue(&arbiter, ticket(1, "head", head_identity, vec![]));
    set_balance(&arbiter, "head", 5);
    set_balance(&arbiter, "wifi", -5);
    let waiter = {
        let arbiter = arbiter.clone();
        std::thread::spawn(move || {
            let grant = arbiter.acquire_within(&request("wifi", "smoke"), None);
            grant.map(drop).is_ok()
        })
    };
    std::thread::sleep(Duration::from_millis(700));
    assert!(
        state(&arbiter).holders.is_empty(),
        "the head's balance is higher"
    );
    set_balance(&arbiter, "wifi", 10);
    assert!(waiter.join().unwrap(), "a higher balance is served first");
    let record = &arbiter.history().unwrap()[0];
    assert_eq!(record.owner, "wifi");
    let reason = record.reason.as_ref().unwrap();
    assert!(
        reason.over.iter().any(|over| over.owner == "head"),
        "{reason:?}"
    );
    stop(head);
}

#[test]
fn a_lease_is_charged_the_time_it_holds() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = arbiter(directory.path());
    let grant = arbiter
        .acquire_within(&request("phy", "run"), None)
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    grant.mark_yielded();
    drop(grant);
    let record = &arbiter.history().unwrap()[0];
    assert_eq!(record.outcome, LeaseOutcome::YieldedToBalance);
    assert!(record.charged_ms >= 300, "{record:?}");
}

#[test]
fn divisible_work_yields_after_its_slice_to_a_waiter_with_a_higher_balance() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = arbiter(directory.path());
    let mut grant = arbiter
        .acquire_within(&on_board(CHIP_A, request("phy", "series")), None)
        .unwrap();
    grant.supervise_with(true, Duration::from_secs(1), Duration::from_secs(60));
    let (child, identity) = other_process();
    queue(
        &arbiter,
        ticket(
            50,
            "wifi",
            identity,
            vec![Claim::board(CHIP_B), Claim::shared(AIR)],
        ),
    );
    set_balance(&arbiter, "wifi", 10);
    std::thread::sleep(Duration::from_millis(2500));
    assert!(!grant.yield_requested(), "a waiter on another board");
    arbiter
        .transaction(|state| {
            state.queue[0].claims = normalize(&[Claim::board(CHIP_A)]);
            Ok(())
        })
        .unwrap();
    set_balance(&arbiter, "phy", 20);
    std::thread::sleep(Duration::from_millis(2500));
    assert!(!grant.yield_requested(), "the waiter's balance is lower");
    set_balance(&arbiter, "phy", -20);
    std::thread::sleep(Duration::from_millis(2500));
    assert!(grant.yield_requested());
    grant.mark_yielded();
    drop(grant);
    stop(child);
}

#[test]
fn indivisible_work_runs_on_while_a_waiter_has_a_higher_balance() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = arbiter(directory.path());
    let mut grant = arbiter
        .acquire_within(&on_board(CHIP_A, request("phy", "flash")), None)
        .unwrap();
    grant.supervise_with(false, Duration::ZERO, Duration::from_secs(60));
    let (child, identity) = other_process();
    queue(
        &arbiter,
        ticket(51, "wifi", identity, vec![Claim::board(CHIP_A)]),
    );
    set_balance(&arbiter, "wifi", 30);
    std::thread::sleep(Duration::from_millis(2500));
    assert!(!grant.yield_requested());
    assert_eq!(state(&arbiter).holders.len(), 1);
    drop(grant);
    stop(child);
}

#[test]
fn status_and_board_report_the_latest_state() {
    let directory = tempfile::tempdir().unwrap();
    // A stand file without boards: status reports attached ports alone.
    let arbiter = arbiter(directory.path());
    arbiter
        .journal()
        .record(
            None,
            BoardEventKind::StartupArtifactWritten {
                path: "/tmp/a".into(),
                sha256: "ab".into(),
                disposition: "Created".into(),
            },
        )
        .unwrap();
    let (holder, identity) = other_process();
    let mut leased = ticket(4, "802154", identity, vec![Claim::board("chip-b")]);
    leased.job = Some(String::from("17-a"));
    hold(&arbiter, leased);
    let status = arbiter.status().unwrap();
    assert_eq!(status.holders[0].owner, "802154");
    // The lease is its job's ticket: the job's phase is read from the queue.
    assert_eq!(status.holders[0].job.as_deref(), Some("17-a"));
    let mut job = crate::jobs::Job::new(
        "802154",
        vec![String::from("run"), String::from("s")],
        "/checkout".into(),
        None,
    );
    job.id = String::from("17-a");
    job.state = crate::jobs::JobState::Started;
    assert_eq!(
        crate::jobs::phase(&job, &status),
        crate::jobs::Phase::Holding
    );
    job.id = String::from("18-b");
    assert_eq!(
        crate::jobs::phase(&job, &status),
        crate::jobs::Phase::Building
    );
    assert_eq!(status.holders[0].claims, "board:chip-b");
    assert_eq!(status.startup_artifacts.len(), 1);
    assert!(
        status
            .devices
            .iter()
            .all(|device| device.firmware.is_none())
    );
    let text = status.to_string();
    assert!(text.contains("802154"), "{text}");
    stop(holder);
    assert!(arbiter.status().unwrap().holders.is_empty());
}

#[test]
fn a_newer_state_schema_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("state.json"), r#"{"schema":4}"#).unwrap();
    let arbiter = arbiter(directory.path());
    assert!(arbiter.status().is_err());
}

/// Set in the child copy of the test binary that holds a supervised lease.
const WATCHDOG_CHILD: &str = "OER_STAND_ARBITER_TEST_WATCHDOG_CHILD";

/// Run `test` in a child copy of this binary holding a one-second
/// indivisible lease, run `granted` once it holds the lease, and return the
/// elapsed time since the grant and the lease's outcome.
fn supervised_child(
    test: &str,
    directory: &std::path::Path,
    granted: impl FnOnce(&Arbiter),
) -> (Duration, LeaseOutcome) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--test-threads=1"])
        .env(WATCHDOG_CHILD, directory)
        .spawn()
        .unwrap();
    let arbiter = Arbiter::at(directory).unwrap();
    while state(&arbiter).holders.is_empty() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the child exited before its lease was granted: {status}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let started = Instant::now();
    granted(&arbiter);
    assert!(child.wait().unwrap().success());
    let history = arbiter.history().unwrap();
    let lease = history
        .iter()
        .find(|record| record.owner == "phy")
        .expect("the child's lease");
    (started.elapsed(), lease.outcome)
}

fn hold_supervised_lease(directory: std::ffi::OsString) {
    let _signals = oer_process::install_signal_handlers().unwrap();
    let arbiter = Arbiter::at(std::path::PathBuf::from(directory)).unwrap();
    let mut grant = arbiter
        .acquire_within(&on_board(HUNG, request("phy", "hung")), None)
        .unwrap();
    grant.supervise_with(false, MIN_SLICE, Duration::from_secs(1));
    // The ordinary cancellation path releases the lease.
    while oer_process::sleep(Duration::from_secs(60)).is_ok() {}
}

#[test]
fn every_lease_is_terminated_at_the_hard_limit() {
    if let Some(directory) = std::env::var_os(WATCHDOG_CHILD) {
        return hold_supervised_lease(directory);
    }
    let directory = tempfile::tempdir().unwrap();
    let (elapsed, outcome) = supervised_child(
        "grant::tests::every_lease_is_terminated_at_the_hard_limit",
        directory.path(),
        |_| {},
    );
    assert!(elapsed >= Duration::from_millis(900), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
    assert_eq!(outcome, LeaseOutcome::HardLimit);
}

#[test]
fn an_older_state_schema_is_refused_without_conversion() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let older = br#"{"schema":2,"next_id":2,"queue":[],"holders":[],"jumped":false}"#;
    std::fs::write(&path, older).unwrap();
    let arbiter = arbiter(directory.path());
    let error = arbiter
        .acquire_within(&on_board(CHIP_B, request("802154", "peer")), None)
        .err()
        .expect("an older state is refused")
        .to_string();
    assert!(error.contains("does not convert"), "{error}");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        older,
        "the state is left as it is"
    );
}

#[test]
fn a_waiting_request_reports_again_only_when_its_position_or_holders_change() {
    let (key, message) = waiting_report(4, "board:AA", 1, "held by phy `run a`", 60_000, 90);
    // The balance and the expected start grow every second while it waits.
    let (same, later) = waiting_report(4, "board:AA", 1, "held by phy `run a`", 61_000, 89);
    assert_eq!(key, same);
    assert_ne!(message, later);
    assert!(message.contains("balance +1m"), "{message}");
    assert_ne!(
        key,
        waiting_report(4, "board:AA", 0, "held by phy `run a`", 61_000, 89).0
    );
    assert_ne!(
        key,
        waiting_report(4, "board:AA", 1, "no conflicting holder", 61_000, 0).0
    );
}

#[test]
fn a_lease_owned_by_no_agent_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = arbiter(directory.path());
    let Err(error) = arbiter.acquire_within(&request("Not An Owner", "run a"), None) else {
        panic!("a lease of no agent was granted");
    };
    assert!(error.is::<oer_stand_owners::NotAnOwner>(), "{error}");
    assert!(state(&arbiter).queue.is_empty() && state(&arbiter).holders.is_empty());
}

#[test]
fn divisible_work_renews_a_lease_after_a_third_of_the_hard_limit() {
    assert!(!renewal_due(Duration::from_secs(19 * 60)));
    assert!(renewal_due(RENEW_AFTER));
    // The step after a renewal has the rest of a fresh lease.
    assert!(HARD_LIMIT - RENEW_AFTER >= Duration::from_secs(40 * 60));
}

#[test]
fn a_whole_stand_lease_without_its_stand_file_is_refused_before_it_queues() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let Err(error) = arbiter.acquire_within(&request("wifi", "run a"), None) else {
        panic!("a whole-stand lease was granted without a stand file to lock its boards");
    };
    let error = error.to_string();
    assert!(
        error.contains("stand.toml does not load, so a whole-stand lease cannot lock its boards"),
        "{error}"
    );
    assert!(state(&arbiter).queue.is_empty() && state(&arbiter).holders.is_empty());
}

#[test]
fn a_board_claim_without_a_mac_is_refused_before_it_queues() {
    let directory = tempfile::tempdir().unwrap();
    let arbiter = Arbiter::at(directory.path()).unwrap();
    let Err(error) = arbiter.acquire_within(&on_board("chip-a", request("phy", "run a")), None)
    else {
        panic!("a board claim without a MAC was granted with no device lock");
    };
    assert!(
        error.to_string().contains("claim `board:chip-a`"),
        "{error}"
    );
    assert!(state(&arbiter).queue.is_empty() && state(&arbiter).holders.is_empty());
}
