use std::time::Duration;

use super::*;

use crate::state::State;

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

fn state(arbiter: &Arbiter) -> State {
    arbiter.transaction(|state| Ok(state.clone())).unwrap()
}

#[test]
fn a_newer_state_schema_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("state.json"), r#"{"schema":4}"#).unwrap();
    let arbiter = arbiter(directory.path());
    assert!(arbiter.status().is_err());
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
    assert!(error.is::<crate::owner::NotAnOwner>(), "{error}");
    assert!(state(&arbiter).queue.is_empty() && state(&arbiter).holders.is_empty());
}

#[test]
fn divisible_work_renews_a_lease_after_a_third_of_the_hard_limit() {
    assert!(!renewal_due(Duration::from_secs(19 * 60)));
    assert!(renewal_due(RENEW_AFTER));
    // The step after a renewal has the rest of a fresh lease.
    assert!(HARD_LIMIT - RENEW_AFTER >= Duration::from_secs(40 * 60));
}
