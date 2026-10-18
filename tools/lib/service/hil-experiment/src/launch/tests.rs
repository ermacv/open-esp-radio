use std::os::unix::fs::PermissionsExt as _;

use super::*;

/// A runner script that records each of its arguments as a run, the way
/// the runner names the runs it creates, and exits with `status`.
fn runner(directory: &Path, status: u8) -> Runner {
    let executable = directory.join("runner");
    std::fs::write(
        &executable,
        format!(
            "#!/bin/sh\nfor run in \"$@\"; do echo \"$run\" >> \"$OER_HIL_RUN_RECEIPT\"; done\n\
             echo \"${{OER_OBSERVER_RECEIPT:-none}} $EXPERIMENT ${{HIDDEN:-gone}}\" > observed\n\
             exit {status}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    Runner {
        executable,
        receipt: None,
    }
}

#[test]
fn a_launch_returns_the_runs_its_runner_names_in_its_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let runner = runner(directory.path(), 3);
    let launched = launch_run(
        &Launch::new(directory.path(), &runner)
            .args(["1-a", "2-b"])
            .env("EXPERIMENT", "arm-a")
            .env_remove("HIDDEN"),
    )
    .unwrap();
    assert_eq!(launched.runs, [RunId::new("1-a"), RunId::new("2-b")]);
    assert_eq!(launched.run().unwrap(), RunId::new("2-b"));
    assert_eq!(launched.status.code(), Some(3));
    assert_eq!(
        std::fs::read_to_string(directory.path().join("observed")).unwrap(),
        "none arm-a gone\n"
    );
}

#[test]
fn a_runner_that_created_no_run_is_an_error_to_whoever_needs_one() {
    let directory = tempfile::tempdir().unwrap();
    let runner = runner(directory.path(), 1);
    let launched = launch_run(&Launch::new(directory.path(), &runner)).unwrap();
    assert!(launched.runs.is_empty());
    assert!(launched.run().is_err());
}
