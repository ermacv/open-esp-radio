use super::{Cleanup, Ending, finish};
use crate::fakes::{Behavior, IMAGE, WAITS, device};
use crate::{Outcome, Scenario, Verdict, run};

#[test]
fn a_device_running_the_image_ends_clean() {
    let link = device(1, Behavior::Answers);
    assert_eq!(
        finish(Some(&link), IMAGE, Ending::default(), WAITS),
        Cleanup::Clean
    );
}

#[test]
fn an_unconfirmed_request_keeps_the_device_from_ending_clean() {
    let link = device(1, Behavior::Answers);
    let ending = Ending { unconfirmed: true };
    assert_eq!(
        finish(Some(&link), IMAGE, ending, WAITS),
        Cleanup::Unconfirmed
    );
}

#[test]
fn a_device_that_does_not_answer_as_the_built_image_is_reported() {
    for link in [
        Some(device(1, Behavior::Mute)),
        Some(device(1, Behavior::Silent)),
        None,
    ] {
        assert_eq!(
            finish(link.as_ref(), IMAGE, Ending::default(), WAITS),
            Cleanup::Unanswered
        );
    }
    let other = device(1, Behavior::Answers);
    assert_eq!(
        finish(Some(&other), IMAGE ^ 1, Ending::default(), WAITS),
        Cleanup::Unanswered
    );
}

#[test]
fn a_cleanup_keeps_a_pass_from_passing_and_never_makes_a_failure_pass() {
    const SCENARIO: Scenario = Scenario::new("test", "test-image", &[]);
    let outcome = |verdict| Outcome {
        case: Some("a"),
        verdict,
    };
    let recovered = Cleanup::Unconfirmed;
    let mut out = Vec::new();
    let passed = run::summary(
        &SCENARIO,
        &[outcome(Verdict::Pass)],
        Some(&recovered),
        &mut out,
    );
    assert_eq!(passed.exit_code(), 2);
    assert!(String::from_utf8(out).unwrap().starts_with("cleanup: "));
    let failed = run::summary(
        &SCENARIO,
        &[outcome(Verdict::Fail("wrong".into()))],
        Some(&recovered),
        &mut Vec::new(),
    );
    assert_eq!(failed, Verdict::Fail("wrong".into()));
    let clean = run::summary(
        &SCENARIO,
        &[outcome(Verdict::Pass)],
        Some(&Cleanup::Clean),
        &mut Vec::new(),
    );
    assert_eq!(clean, Verdict::Pass);
}

#[test]
fn a_lease_the_lab_ended_unconfirmed_keeps_a_clean_device_from_passing() {
    assert_eq!(Cleanup::Clean.released(Ok(())), Cleanup::Clean);
    let problem = Cleanup::Clean
        .released(Err(
            "board:02:00:00:00:00:0a (its ROM did not answer)".to_owned()
        ))
        .problem()
        .unwrap();
    assert!(
        problem.contains("left clean") && problem.contains("its ROM did not answer"),
        "{problem}"
    );
}
