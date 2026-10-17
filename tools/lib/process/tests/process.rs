#![cfg(target_os = "linux")]

use oer_process::{self as process, owned};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    time::{Duration, Instant},
};

struct Fixture {
    _directory: tempfile::TempDir,
    executable: PathBuf,
}
fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let directory = tempfile::Builder::new()
            .prefix("oer process дерево ")
            .tempdir()
            .unwrap();
        let executable = directory.path().join("process fixture");
        let status = Command::new("rustc")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process_tree.rs"))
            .arg("-o")
            .arg(&executable)
            .status()
            .unwrap();
        assert!(status.success());
        Fixture {
            _directory: directory,
            executable,
        }
    })
}
fn wait_for_pids(marker: &Path) -> Vec<u32> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = fs::read_to_string(marker) {
            let pids: Vec<u32> = text
                .split_whitespace()
                .map(|pid| pid.parse().unwrap())
                .collect();
            if pids.len() == 2 {
                return pids;
            }
        }
        assert!(
            Instant::now() < deadline,
            "process fixture did not become ready"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
/// The live processes named `name` whose parent is `parent`.
fn children_of(parent: u32, name: &str) -> Vec<u32> {
    fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                let mut fields = stat.rsplit_once(") ").unwrap().1.split(' ');
                let state = fields.next().unwrap();
                state != "Z" && fields.next().unwrap().parse::<u32>().unwrap() == parent
            }) && fs::read_to_string(format!("/proc/{pid}/comm"))
                .is_ok_and(|comm| comm.trim_end() == name)
        })
        .collect()
}

fn assert_stopped(pids: &[u32]) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let running = pids.iter().any(|pid| {
            fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                let state = stat.rsplit_once(") ").unwrap().1.chars().next().unwrap();
                state != 'Z' && state != 'X'
            })
        });
        if !running {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "owned descendants are still running: {pids:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn capture_drains_both_pipes_beyond_pipe_capacity() {
    let output = process::capture(Command::new(&fixture().executable).arg("output")).unwrap();
    assert_eq!(output.stdout, vec![b'x'; 2 * 1024 * 1024]);
    assert_eq!(output.stderr, vec![b'x'; 2 * 1024 * 1024]);
}
#[test]
fn dropping_owned_child_stops_its_descendants() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("pids");
    let child =
        owned::Child::spawn(Command::new(&fixture().executable).arg("tree").arg(&marker)).unwrap();
    let pids = wait_for_pids(&marker);
    drop(child);
    assert_stopped(&pids);
}
#[test]
fn cancellation_harness() {
    let Some(executable) = std::env::var_os("OER_PROCESS_TEST_EXECUTABLE") else {
        return;
    };
    let marker = std::env::var_os("OER_PROCESS_TEST_MARKER").unwrap();
    let _signals = process::install_signal_handlers().unwrap();
    let error = process::capture(Command::new(&executable).arg("tree").arg(marker)).unwrap_err();
    assert!(error.to_string().contains("cancelled by signal"), "{error}");
    assert!(process::is_cancelled(&*error));
    assert!(process::capture(Command::new(&executable).arg("input")).is_err());
    process::cleanup(|| process::capture(Command::new(&executable).arg("input"))).unwrap();
    assert!(
        process::check_cancelled().is_err(),
        "cleanup policy escaped its scope"
    );
    if let Some(marker) = std::env::var_os("OER_PROCESS_SUPERVISOR_STOPPED") {
        // Model a supervisor finishing bounded cleanup after its child stops.
        std::thread::sleep(Duration::from_millis(1200));
        fs::write(marker, "cleaned").unwrap();
    }
}
#[test]
fn signals_cancel_capture_and_stop_descendants() {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("pids");
        let mut harness = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "cancellation_harness", "--nocapture"])
            .env("OER_PROCESS_TEST_EXECUTABLE", &fixture().executable)
            .env("OER_PROCESS_TEST_MARKER", &marker)
            .spawn()
            .unwrap();
        let pids = wait_for_pids(&marker);
        // SAFETY: this PID is the live child owned by this test, and the signal
        // exercises its installed handler without signalling the test runner.
        assert_eq!(
            unsafe { libc::kill(harness.id().try_into().unwrap(), signal) },
            0
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = harness.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                harness.kill().unwrap();
                harness.wait().unwrap();
                panic!("cancelled capture did not release its readers");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_stopped(&pids);
    }
}

#[test]
fn a_killed_owner_takes_its_child_groups_with_it() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("pids");
    let mut harness = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "cancellation_harness", "--nocapture"])
        .env("OER_PROCESS_TEST_EXECUTABLE", &fixture().executable)
        .env("OER_PROCESS_TEST_MARKER", &marker)
        .spawn()
        .unwrap();
    let pids = wait_for_pids(&marker);
    // SIGKILL leaves the owner no chance to clean up: its guardian must.
    harness.kill().unwrap();
    harness.wait().unwrap();
    assert_stopped(&pids);
}

/// Holds as many owned children as the guardian tracks, marks the moment
/// one more was refused, and waits to be killed.
#[test]
fn guardian_capacity_harness() {
    let Some(marker) = std::env::var_os("OER_PROCESS_TEST_CAPACITY_MARKER") else {
        return;
    };
    let sleeper = || {
        let mut command = Command::new("sleep");
        command.arg("600");
        command
    };
    let children: Vec<_> = (0..owned::MAX_LIVE_CHILDREN)
        .map(|_| owned::Child::spawn(&mut sleeper()).unwrap())
        .collect();
    let Err(error) = owned::Child::spawn(&mut sleeper()) else {
        panic!("a child beyond the guardian's capacity was spawned unguarded");
    };
    assert!(
        error.to_string().contains("would outlive a lost owner"),
        "{error}"
    );
    fs::write(&marker, "refused").unwrap();
    std::thread::sleep(Duration::from_secs(600));
    drop(children);
}

#[test]
fn a_child_beyond_the_guardians_capacity_is_refused_and_the_rest_are_guarded() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("pids");
    let mut harness = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "guardian_capacity_harness", "--nocapture"])
        .env("OER_PROCESS_TEST_CAPACITY_MARKER", &marker)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker.exists() {
        assert!(
            Instant::now() < deadline,
            "the harness did not reach capacity"
        );
        assert!(harness.try_wait().unwrap().is_none(), "the harness failed");
        std::thread::sleep(Duration::from_millis(10));
    }
    // The refused child was stopped and reaped: only the guarded ones remain
    // beside the guardian, which the harness forked too.
    let pids = children_of(harness.id(), "sleep");
    assert_eq!(pids.len(), owned::MAX_LIVE_CHILDREN);
    harness.kill().unwrap();
    harness.wait().unwrap();
    assert_stopped(&pids);
}

#[test]
fn a_timeout_stops_the_command_and_names_its_limit() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("pids");
    let error = process::run_with_timeout(
        Command::new(&fixture().executable).arg("tree").arg(&marker),
        Duration::from_secs(1),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("exceeded its 1 s limit"),
        "{error}"
    );
    assert_stopped(&wait_for_pids(&marker));
}

#[test]
fn nested_supervisor_gets_its_explicit_shutdown_grace() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("pids");
    let stopped = directory.path().join("supervisor cleaned");
    let supervisor = owned::Child::spawn_with_shutdown_grace(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "cancellation_harness", "--nocapture"])
            .env("OER_PROCESS_TEST_EXECUTABLE", &fixture().executable)
            .env("OER_PROCESS_TEST_MARKER", &marker)
            .env("OER_PROCESS_SUPERVISOR_STOPPED", &stopped),
        Duration::from_secs(4),
    )
    .unwrap();
    let pids = wait_for_pids(&marker);
    drop(supervisor);
    assert_eq!(fs::read_to_string(stopped).unwrap(), "cleaned");
    assert_stopped(&pids);
}

#[test]
fn failed_leader_does_not_leave_descendants_holding_capture_pipes() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("pids");
    let error = process::capture(
        Command::new(&fixture().executable)
            .arg("tree-exit")
            .arg(&marker),
    )
    .unwrap_err();
    assert!(error.to_string().contains("exit status: 23"), "{error}");
    assert_stopped(&wait_for_pids(&marker));
}

#[test]
fn capture_closes_stdin_for_noninteractive_probes() {
    let output = process::capture(
        Command::new(&fixture().executable)
            .arg("input")
            .stdin(std::process::Stdio::piped()),
    )
    .unwrap();
    assert_eq!(output.stdout, b"stdin EOF\n");
}

#[test]
fn deadline_stops_descendants_and_releases_output_readers() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("pids");
    let started = Instant::now();
    let error = process::output(
        Command::new(&fixture().executable).arg("tree").arg(&marker),
        Some(Duration::from_millis(300)),
    )
    .unwrap_err();
    assert!(error.is::<owned::DeadlineExceeded>());
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_stopped(&wait_for_pids(&marker));
}

#[test]
fn background_deadline_includes_time_before_waiting() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("pids");
    let mut child = owned::Child::spawn_with_shutdown_grace(
        Command::new(&fixture().executable).arg("tree").arg(&marker),
        Duration::from_millis(20),
    )
    .unwrap()
    .with_timeout(Duration::from_millis(150));
    let pids = wait_for_pids(&marker);
    std::thread::sleep(Duration::from_millis(200));
    let error = child.wait().unwrap_err();
    assert!(error.is::<owned::DeadlineExceeded>());
    assert_stopped(&pids);
}

#[test]
fn a_wrapper_exits_with_its_childs_code_or_128_plus_the_signal() {
    let status = owned::Child::spawn(Command::new("sh").args(["-c", "exit 37"]))
        .unwrap()
        .wait_forwarding_cancellation()
        .unwrap();
    assert_eq!(process::exit_code(status), std::process::ExitCode::from(37));
    let status = Command::new("sh")
        .args(["-c", "kill -TERM $$"])
        .status()
        .unwrap();
    assert_eq!(
        process::exit_code(status),
        std::process::ExitCode::from(128 + 15)
    );
}
