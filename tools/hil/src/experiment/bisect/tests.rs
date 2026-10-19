use super::*;

#[test]
fn the_exit_codes_are_git_bisect_runs() {
    // 0 good, 1 bad, 125 skip, above 127 an abort (git-bisect(1), "bisect run").
    let run = String::from("7");
    let why = String::from("why");
    assert_eq!(Step::Pass { run: run.clone() }.code(), 0);
    assert_eq!(Step::Fail { run }.code(), 1);
    assert_eq!(Step::Skip { why: why.clone() }.code(), 125);
    let abort = Step::Unjudged { why }.code();
    assert!(abort > 127, "{abort}");
}

#[test]
fn a_link_failure_is_told_from_a_compile_failure() {
    assert_eq!(
        build_failure(
            "error: linking with `rust-lld` failed\n rust-lld: error: R_RISCV_JAL out of range"
        ),
        "the image does not link"
    );
    assert_eq!(
        build_failure("error[E0425]: cannot find value `x` in this scope"),
        "the image does not build"
    );
}

/// A completed run `7` below `runs` of one scenario `s` on the Bluetooth GATT
/// image: one repetition that ended `outcome` with `failure`, or, for a
/// blocked scenario, no repetition and the failure that blocked it.
pub(crate) fn run_with(outcome: Outcome, failure: Option<(FailureKind, &str)>, runs: &Path) -> Run {
    use oer_hil_model::schema::image::ImageClass;
    use oer_hil_runs::format::run::Failure;
    use oer_hil_runs::format::run::RunState;
    use oer_hil_runs::format::run::ScenarioResult;
    use oer_hil_runs::run::test_support::repetition;
    use oer_hil_runs::run::test_support::write_run;
    let directory = runs.join("7");
    let failure = failure.map(|(kind, message)| Failure::new(kind, message));
    let scenario = match (outcome, failure) {
        (Outcome::Blocked, Some(failure)) => {
            ScenarioResult::blocked("s".into(), ImageClass::SystemWatchdog, 1, failure)
        }
        (outcome, failure) => ScenarioResult::from_repetitions(
            "s".into(),
            ImageClass::SystemWatchdog,
            1,
            vec![repetition("s", 1, outcome, failure, Vec::new())],
        ),
    };
    write_run(&directory, 0, RunState::Completed, vec![scenario], |_| {});
    Run::load(&directory).unwrap()
}

#[test]
fn a_run_is_judged_by_its_outcome_and_its_build_log() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        judge(&run_with(Outcome::Passed, None, directory.path())),
        Step::Pass { run: "7".into() }
    );
    assert_eq!(
        judge(&run_with(
            Outcome::Failed,
            Some((FailureKind::Scenario, "missed")),
            directory.path()
        )),
        Step::Fail { run: "7".into() }
    );
    // The scenario a failed image build blocked is skipped, not bad.
    let log = directory
        .path()
        .join("7/firmware/system-watchdog/build.log");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, "rust-lld: error: undefined symbol: foo\n").unwrap();
    assert_eq!(
        judge(&run_with(
            Outcome::Blocked,
            Some((FailureKind::ImageBuild, "runtime build failed")),
            directory.path()
        )),
        Step::Skip {
            why: String::from("the image does not link (run 7)")
        }
    );
    // A broken run for another reason judges nothing and stops the search.
    let step = judge(&run_with(
        Outcome::Broken,
        Some((FailureKind::Infrastructure, "port vanished")),
        directory.path(),
    ));
    assert!(matches!(step, Step::Unjudged { .. }), "{step:?}");
}

#[test]
fn an_unreadable_run_names_its_path_and_why() {
    let directory = tempfile::tempdir().unwrap();
    let Step::Unjudged { why } = judge_run(directory.path()) else {
        panic!("an empty directory was judged");
    };
    assert!(
        why.contains(&directory.path().display().to_string()),
        "{why}"
    );
    assert!(why.contains("no manifest"), "{why}");
    std::fs::write(directory.path().join("manifest.json"), "{").unwrap();
    let Step::Unjudged { why } = judge_run(directory.path()) else {
        panic!("an unreadable manifest was judged");
    };
    assert!(why.contains("does not read: "), "{why}");
    assert!(!why.contains("no manifest"), "{why}");
}

#[test]
fn a_step_line_names_the_commit_and_the_run() {
    let commit = "0123456789abcdef";
    assert_eq!(
        Step::Pass { run: "7".into() }.line(commit),
        "0123456789ab pass (run 7)"
    );
    assert_eq!(
        Step::Skip { why: "x".into() }.line(commit),
        "0123456789ab skip: x"
    );
}
