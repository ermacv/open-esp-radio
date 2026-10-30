//! The tool's process group outlives no ending of the tool. The test binary
//! plays every process: `role` is a test that does nothing unless [`ROLE`]
//! names a part, so the tool is this binary as `parent`, the scenario as
//! `scenario` and the scenario's own child as `descendant`.

use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

use rustix::process::{Pid, Signal, kill_process, test_kill_process};

const ROLE: &str = "OER_HIL_CLI_TEST_ROLE";
const DIRECTORY: &str = "OER_HIL_CLI_TEST_DIRECTORY";

/// This binary running only the `role` test as `role`.
fn this_binary(role: &str, directory: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "tests::role", "--nocapture", "--test-threads=1"])
        .env(ROLE, role)
        .env(DIRECTORY, directory);
    command
}

#[test]
fn role() {
    let Some(role) = std::env::var_os(ROLE) else {
        return;
    };
    let directory = Path::new(&std::env::var_os(DIRECTORY).unwrap()).to_owned();
    match role.to_str().unwrap() {
        "parent" => {
            let _signals = oer_os_process::install_signal_handlers().unwrap();
            let _ = super::owned(
                &mut this_binary("scenario", &directory),
                Duration::from_secs(1),
            );
        }
        "scenario" => {
            let mut descendant = this_binary("descendant", &directory).spawn().unwrap();
            std::fs::write(
                directory.join("pids.tmp"),
                format!("{} {}", std::process::id(), descendant.id()),
            )
            .unwrap();
            std::fs::rename(directory.join("pids.tmp"), directory.join("pids")).unwrap();
            std::thread::sleep(Duration::from_secs(120));
            let _ = descendant.wait();
        }
        "descendant" => std::thread::sleep(Duration::from_secs(120)),
        other => panic!("unknown role {other}"),
    }
}

/// `condition` within `limit`.
fn within(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    condition()
}

/// Start the tool, wait for its scenario and descendant, end the tool with
/// `signal` and require that both are gone.
fn ending_the_tool_ends_its_scenario(signal: Signal) {
    let directory = tempfile::tempdir().unwrap();
    let mut tool = this_binary("parent", directory.path()).spawn().unwrap();
    let pids = directory.path().join("pids");
    assert!(within(Duration::from_secs(30), || pids.exists()));
    let pids = std::fs::read_to_string(pids)
        .unwrap()
        .split(' ')
        .map(|pid| Pid::from_raw(pid.parse().unwrap()).unwrap())
        .collect::<Vec<_>>();
    kill_process(Pid::from_raw(tool.id() as i32).unwrap(), signal).unwrap();
    assert!(within(Duration::from_secs(10), || tool
        .try_wait()
        .unwrap()
        .is_some()));
    for pid in pids {
        assert!(
            within(Duration::from_secs(10), || test_kill_process(pid).is_err()),
            "{pid:?} outlived the tool ended by {signal:?}"
        );
    }
}

#[test]
fn a_stopped_tool_stops_its_scenario_and_descendants() {
    ending_the_tool_ends_its_scenario(Signal::TERM);
}

#[test]
fn a_killed_tool_leaves_no_scenario_or_descendant() {
    ending_the_tool_ends_its_scenario(Signal::KILL);
}
