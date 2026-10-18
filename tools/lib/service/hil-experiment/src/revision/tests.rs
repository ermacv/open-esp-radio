use super::*;

use oer_hil_schema::run::Outcome;

#[test]
fn a_revision_runs_on_this_checkout_s_runner_only_for_the_same_wire() {
    assert_eq!(Runner::for_wire("wire", Some("wire")), Runner::Current);
    assert_eq!(Runner::for_wire("wire", Some("other")), Runner::Revision);
    // A revision older than the keyed protocol names no wire.
    assert_eq!(Runner::for_wire("wire", None), Runner::Revision);
}

#[test]
fn the_checked_out_tree_s_lock_selects_the_runner() {
    let directory = tempfile::tempdir().unwrap();
    let worktree = directory.path().join("worktree");
    fs::create_dir_all(worktree.join("hil/protocol")).unwrap();
    let revision = Revision {
        checkout: directory.path(),
        owner: "experiment-test",
        current: String::from("framing 1\n"),
        runner: launch::Runner {
            executable: PathBuf::from("runner"),
            receipt: None,
        },
        store: RunStore::at(directory.path().join("store")),
        worktree: worktree.clone(),
        arbiter: directory.path().join("arbiter"),
        runs: directory.path().join("runs"),
    };
    assert_eq!(revision.runner(), Runner::Revision);
    fs::write(worktree.join(MESSAGES_LOCK), "framing 2\n").unwrap();
    assert_eq!(revision.runner(), Runner::Revision);
    fs::write(worktree.join(MESSAGES_LOCK), "framing 1\n").unwrap();
    assert_eq!(revision.runner(), Runner::Current);
}

#[test]
fn this_tree_names_its_wire_and_an_older_tree_none() {
    let lock = messages_lock(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    assert!(lock.is_some_and(|lock| lock.contains("\nframing ")));
    assert_eq!(messages_lock(tempfile::tempdir().unwrap().path()), None);
}

#[test]
fn enclosing_lease_reaches_current_runner_launches() {
    crate::lease_tests::within_enclosing_lease(
        "revision::tests::enclosing_lease_reaches_current_runner_launches",
        |directory, runner| {
            let root = crate::lease_tests::repository(directory);
            let store = RunStore::at(directory.join("store"));
            crate::bisect::tests::run_with(Outcome::Passed, None, &store.runs());
            let revision = Revision {
                checkout: &root,
                owner: "experiment-test",
                current: String::new(),
                runner,
                store,
                worktree: root.clone(),
                arbiter: directory.join("private-arbiter"),
                runs: directory.join("runs"),
            };
            let runs = revision.store.runs();
            assert_eq!(
                revision
                    .with_current_runner(&[String::from("s")], &[])
                    .unwrap(),
                Ran::Run {
                    id: String::from("7"),
                    directory: runs.join("7"),
                }
            );
        },
    );
}

/// A revision in `directory`, its worktree and store below it.
fn revision(directory: &Path) -> Revision<'_> {
    Revision {
        checkout: directory,
        owner: "experiment-test",
        current: String::from("framing 1\n"),
        runner: launch::Runner {
            executable: PathBuf::from("runner"),
            receipt: None,
        },
        store: RunStore::at(directory.join("store")),
        worktree: directory.join("worktree"),
        arbiter: directory.join("arbiter"),
        runs: RunStore::at(directory.join("store")).revision_runs(),
    }
}

#[test]
fn an_own_runner_reads_the_private_arbiter_and_owner_under_every_name() {
    let directory = tempfile::tempdir().unwrap();
    let revision = revision(directory.path());
    let runner = launch::Runner {
        executable: PathBuf::from("runner"),
        receipt: None,
    };
    let launch = revision.own_runner_launch(&runner, &[String::from("s")], &[String::from("-x")]);
    assert_eq!(launch.checkout, revision.worktree);
    assert_eq!(launch.arguments, ["run", "s", "-x"]);
    let value = |name: &str| {
        launch
            .environment
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    for name in ["OER_STAND_ARBITER_DIR", "OER_HIL_ARBITER_DIR"] {
        assert_eq!(value(name), Some(revision.arbiter.clone().into()), "{name}");
    }
    for name in ["OER_STAND_OWNER", "OER_HIL_OWNER"] {
        assert_eq!(value(name), Some("experiment-test".into()), "{name}");
    }
}

/// A run directory `id` below `parent` with a suite report of `scenarios`.
fn suite(parent: &Path, id: &str, scenarios: &str) -> PathBuf {
    let directory = parent.join(id);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join(SUITE),
        format!(r#"{{"schema":3,"outcome":"failed","scenarios":[{scenarios}]}}"#),
    )
    .unwrap();
    directory
}

#[test]
fn a_run_kept_below_the_worktree_moves_into_the_store_s_revision_runs() {
    let directory = tempfile::tempdir().unwrap();
    let revision = revision(directory.path());
    // An older runner keeps its runs per chip below the worktree.
    let old = suite(
        &revision.worktree.join("target/hil/esp32s31/runs"),
        "41",
        r#"{"schema":1,"scenario":"a","outcome":"passed","repetitions":[]},
           {"schema":1,"scenario":"b","outcome":"failed","repetitions":[]}"#,
    );
    let kept = revision.keep("41").unwrap();
    assert_eq!(kept, revision.runs.join("41"));
    assert!(kept.starts_with(revision.store.root()));
    assert!(!old.exists());
    assert_eq!(
        outcomes(&kept).unwrap(),
        [
            ScenarioOutcome {
                scenario: String::from("a"),
                outcome: Outcome::Passed,
            },
            ScenarioOutcome {
                scenario: String::from("b"),
                outcome: Outcome::Failed,
            },
        ]
    );
    // A run in the shared store stays there.
    let shared = suite(&revision.store.runs(), "42", "");
    assert_eq!(revision.keep("42").unwrap(), shared);
    assert!(revision.keep("43").is_err());
}

fn report(result: RevisionResult) -> Report {
    Report {
        schema: REPORT_SCHEMA,
        revision: String::from("baseline"),
        commit: String::from("0123456789abcdef"),
        subject: String::from("subject"),
        layout_seed: None,
        runner: Runner::Revision,
        result,
    }
}

fn ran(outcomes: &[Outcome]) -> RevisionResult {
    RevisionResult::Ran {
        run: String::from("7"),
        directory: PathBuf::from("runs/7"),
        scenarios: outcomes
            .iter()
            .enumerate()
            .map(|(index, &outcome)| ScenarioOutcome {
                scenario: format!("s{index}"),
                outcome,
            })
            .collect(),
    }
}

#[test]
fn the_report_records_each_scenario_and_exits_by_the_worst() {
    assert_eq!(report(ran(&[Outcome::Passed, Outcome::Passed])).code(), 0);
    assert_eq!(report(ran(&[Outcome::Passed, Outcome::Failed])).code(), 1);
    // A scenario that judged nothing outranks a failure.
    assert_eq!(report(ran(&[Outcome::Failed, Outcome::Blocked])).code(), 2);
    assert_eq!(report(RevisionResult::RunnerUnavailable).code(), 2);
    let report = report(ran(&[Outcome::Passed, Outcome::Failed]));
    assert_eq!(
        summary(&report),
        "baseline at 0123456789ab [own runner]  subject\nrun 7:\n  passed s0\n  failed s1\n"
    );
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["runner"], "revision");
    assert_eq!(json["result"], "ran");
    assert_eq!(json["scenarios"][1]["outcome"], "failed");
    assert_eq!(serde_json::from_value::<Report>(json).unwrap(), report);
}

#[test]
fn a_moved_tree_keeps_its_files_and_links() {
    let directory = tempfile::tempdir().unwrap();
    let from = directory.path().join("from");
    fs::create_dir_all(from.join("scenarios/a")).unwrap();
    fs::write(from.join("suite.json"), "{}").unwrap();
    fs::write(from.join("scenarios/a/uart.log"), "log").unwrap();
    std::os::unix::fs::symlink("suite.json", from.join("link")).unwrap();
    let to = directory.path().join("to");
    // The copy a move onto another file system makes.
    copy_tree(&from, &to).unwrap();
    assert_eq!(
        fs::read_to_string(to.join("scenarios/a/uart.log")).unwrap(),
        "log"
    );
    assert_eq!(
        fs::read_link(to.join("link")).unwrap(),
        Path::new("suite.json")
    );
    // A move on one file system renames.
    let moved = directory.path().join("moved");
    move_directory(&from, &moved).unwrap();
    assert!(!from.exists());
    assert_eq!(fs::read_to_string(moved.join("suite.json")).unwrap(), "{}");
}

#[test]
fn a_move_across_file_systems_leaves_no_partial_tree_when_the_copy_fails() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().unwrap();
    let from = directory.path().join("from");
    fs::create_dir_all(from.join("scenarios")).unwrap();
    fs::write(from.join("suite.json"), "{}").unwrap();
    let unreadable = from.join("scenarios/uart.log");
    fs::write(&unreadable, "log").unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
    // A root test process reads the file anyway: nothing to check then.
    if fs::read(&unreadable).is_ok() {
        return;
    }
    let to = directory.path().join("store/41");
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    assert!(move_across(&from, &to).is_err());
    assert!(!to.exists());
    assert!(!directory.path().join("store/.41.partial").exists());
    assert_eq!(fs::read_to_string(from.join("suite.json")).unwrap(), "{}");
    // Once the file reads, the move completes and removes the source.
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o600)).unwrap();
    move_across(&from, &to).unwrap();
    assert!(!from.exists());
    assert_eq!(
        fs::read_to_string(to.join("scenarios/uart.log")).unwrap(),
        "log"
    );
}
