use super::*;

/// Probe `len` commits whose first bad one is `first_bad`, `broken` ones
/// broken, recording the probed indices.
fn bisect(len: usize, first_bad: usize, broken: &[usize]) -> (Found, Vec<usize>) {
    let mut probed = Vec::new();
    let found = search(len, |index| {
        probed.push(index);
        Ok(if broken.contains(&index) {
            Probe::Broken
        } else if index >= first_bad {
            Probe::Bad
        } else {
            Probe::Good
        })
    })
    .unwrap();
    (found, probed)
}

#[test]
fn the_search_finds_every_first_bad_commit_in_logarithmic_steps() {
    for len in 1..40 {
        for first_bad in 0..len {
            let (found, probed) = bisect(len, first_bad, &[]);
            assert_eq!(found, Found::FirstBad(first_bad), "{len} {first_bad}");
            assert!(probed.len() <= len.ilog2() as usize + 1, "{len} {probed:?}");
            // The known bad commit is never run again.
            assert!(!probed.contains(&(len - 1)));
        }
    }
}

#[test]
fn a_broken_commit_is_neither_good_nor_bad() {
    // A broken commit away from the boundary: the search steps around it.
    let (found, probed) = bisect(16, 9, &[5, 7]);
    assert_eq!(found, Found::FirstBad(9));
    assert!(probed.contains(&7));
    // Broken commits right before the first bad one hide which is first.
    let (found, _) = bisect(16, 9, &[8, 7]);
    assert_eq!(found, Found::Ambiguous(7..=9));
    let (found, _) = bisect(4, 2, &[1]);
    assert_eq!(found, Found::Ambiguous(1..=2));
    // Every commit broken but the known bad one.
    let (found, _) = bisect(3, 2, &[0, 1]);
    assert_eq!(found, Found::Ambiguous(0..=2));
}

#[test]
fn a_step_that_judges_nothing_ends_the_search() {
    let error = search(8, |_| Err("stand quarantined".into())).unwrap_err();
    assert_eq!(error.to_string(), "stand quarantined");
    assert!(search(0, |_| Ok(Probe::Good)).is_err());
}

#[test]
fn a_link_failure_is_told_from_a_compile_failure() {
    assert_eq!(
        build_failure(
            "error: linking with `rust-lld` failed\n rust-lld: error: R_RISCV_JAL out of range"
        ),
        Broken::DoesNotLink
    );
    assert_eq!(
        build_failure("error[E0425]: cannot find value `x` in this scope"),
        Broken::DoesNotBuild
    );
}

/// A completed run `7` below `runs` of one scenario `s` on the Bluetooth GATT
/// image: one repetition that ended `outcome` with `failure`, or, for a
/// blocked scenario, no repetition and the failure that blocked it.
pub(crate) fn run_with(outcome: Outcome, failure: Option<(FailureKind, &str)>, runs: &Path) -> Run {
    use oer_hil_run_bundle::run::test_support::repetition;
    use oer_hil_run_bundle::run::test_support::write_run;
    use oer_hil_run_bundle_format::run::Failure;
    use oer_hil_run_bundle_format::run::RunState;
    use oer_hil_run_bundle_format::run::ScenarioResult;
    use oer_hil_schema::image::ImageClass;
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
        Ok(Verdict::Good { run: "7".into() })
    );
    assert_eq!(
        judge(&run_with(
            Outcome::Failed,
            Some((FailureKind::Scenario, "missed")),
            directory.path()
        )),
        Ok(Verdict::Bad { run: "7".into() })
    );
    // The suite of a run whose image did not build failed; the scenario
    // it blocked is broken, not bad.
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
        Ok(Verdict::Broken {
            why: Broken::DoesNotLink,
            run: Some("7".into())
        })
    );
    // A broken run for another reason judges nothing.
    assert!(
        judge(&run_with(
            Outcome::Broken,
            Some((FailureKind::Infrastructure, "port vanished")),
            directory.path()
        ))
        .is_err()
    );
}

#[test]
fn the_report_names_its_steps_and_conclusion() {
    let step = |commit: &str, verdict| Step {
        commit: commit.into(),
        subject: String::from("subject"),
        runner: Runner::Current,
        verdict,
    };
    let report = Report {
        schema: REPORT_SCHEMA,
        scenario: String::from("s"),
        layout_seed: None,
        good: String::from("a"),
        bad: String::from("d"),
        commits: vec!["b".into(), "c".into(), "d".into()],
        steps: vec![
            step("b", Verdict::Good { run: "1".into() }),
            step(
                "c",
                Verdict::Broken {
                    why: Broken::DoesNotLink,
                    run: Some("2".into()),
                },
            ),
        ],
        conclusion: Some(Conclusion::Ambiguous {
            candidates: vec!["c".into(), "d".into()],
        }),
    };
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["steps"][1]["verdict"], "broken");
    assert_eq!(json["steps"][1]["why"], "does-not-link");
    assert_eq!(json["steps"][1]["runner"], "current");
    assert_eq!(json["conclusion"]["result"], "ambiguous");
    let text = summary(&report);
    assert!(text.contains("c broken: does not link (run 2)"), "{text}");
    assert!(text.contains("hide the first bad commit among"), "{text}");
}

#[test]
fn an_unreadable_run_names_its_path_and_why() {
    let directory = tempfile::tempdir().unwrap();
    let error = judge_run(directory.path()).unwrap_err().to_string();
    assert!(
        error.contains(&directory.path().display().to_string()),
        "{error}"
    );
    assert!(error.contains("no manifest"), "{error}");
    std::fs::write(directory.path().join("manifest.json"), "{").unwrap();
    let error = judge_run(directory.path()).unwrap_err().to_string();
    assert!(error.contains("does not read: "), "{error}");
    assert!(!error.contains("no manifest"), "{error}");
}
