#![cfg(unix)]
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
fn fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("hil/schema")).unwrap();
    // The wrapper links every chip's local runs into the shared store; the
    // fixture has no chip profile and keeps the store inside the fixture.
    fs::create_dir_all(root.join("platform")).unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::write(
        root.join("hil/schema/observer-inputs.json"),
        r#"{"build":{"profile":"debug"}}"#,
    )
    .unwrap();
    executable(
        &root.join("cargo"),
        r#"#!/bin/sh
printf '%s\n' "$@" > "$FIXTURE_ROOT/cargo-args"
printf '%s' "${CARGO_NET_OFFLINE-unset}" > "$FIXTURE_ROOT/offline"
printf '{"reason":"compiler-artifact","target":{"name":"oer-hil-runner"},"executable":"%s/runner"}\n' "$FIXTURE_ROOT"
"#,
    );
    executable(
        &root.join("runner"),
        r#"#!/bin/sh
if [ "$1" = --observer-build ]; then
    printf '{"schema":3,"resolved":{"packages":[]}}\n'
    exit 0
fi
if [ "$1" = exit ]; then exit 37; fi
trap 'sleep 2; echo cleaned > "$FIXTURE_ROOT/cleaned"; exit 42' TERM
echo $$ > "$FIXTURE_ROOT/pid"
while :; do sleep 0.1; done
"#,
    );
    directory
}
fn wrapper(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oer-hil-cli"));
    command
        .arg("--root")
        .arg(root)
        .env("CARGO", root.join("cargo"))
        .env("OER_HIL_STORE", root.join("store"))
        // The wrapper is a real binary, not a test harness: keep its arbiter
        // inside the fixture too.
        .env("OER_STAND_ARBITER_DIR", root.join("arbiter"))
        .env("FIXTURE_ROOT", root);
    command
}

#[test]
fn resolved_owner_reaches_the_runner_without_losing_enclosing_authority() {
    const ROLE: &str = "OER_HIL_OWNER_TEST_ROLE";
    const EXPECTED: &str = "OER_HIL_OWNER_TEST_EXPECTED";
    const TEST: &str = "resolved_owner_reaches_the_runner_without_losing_enclosing_authority";

    let enclosing = oer_process::Context::default()
        .with(oer_stand_owners::OWNER_KEY, "enclosing-owner")
        .with(oer_stand_arbiter::LEASE_KEY, "enclosing-lease")
        .with(oer_stand_arbiter::jobs::JOB_KEY, "enclosing-job")
        .with("device:/fixture/device.lock", "delegated-capability");
    if std::env::var_os(ROLE).is_some() {
        let owner = std::env::var(EXPECTED).unwrap();
        assert_eq!(
            oer_process::Context::current().unwrap(),
            &enclosing.with(oer_stand_owners::OWNER_KEY, &owner),
        );
        assert_eq!(
            oer_stand_owners::from_environment().unwrap().as_str(),
            owner
        );
        assert_eq!(std::env::var(oer_stand_owners::OWNER_ENV).unwrap(), owner);
        return;
    }

    let directory = fixture();
    let root = directory.path();
    executable(
        &root.join("runner"),
        &format!(
            "#!/bin/sh\n\
         if [ \"$1\" = --observer-build ]; then\n\
         printf '{{\"schema\":3,\"resolved\":{{\"packages\":[]}}}}\\n'\n\
         exit 0\nfi\n\
         export {ROLE}=runner\n\
         exec \"{}\" --exact {TEST} --test-threads=1\n",
            std::env::current_exe().unwrap().display(),
        ),
    );
    for (args, owner) in [
        (vec!["--owner", "other", "owner-probe"], "other"),
        (vec!["owner-probe", "--owner=other"], "other"),
        (vec!["owner-probe"], "enclosing-owner"),
    ] {
        let mut command = wrapper(root);
        command
            .args(args)
            .env(EXPECTED, owner)
            .env(oer_stand_owners::OWNER_ENV, "env-owner");
        enclosing.apply(&mut command).unwrap();
        oer_process::run_with_timeout(&mut command, Duration::from_secs(15)).unwrap();
    }
}
#[test]
fn invalid_explicit_owner_is_refused_before_runner_or_job_creation() {
    for args in [
        vec!["--owner", "Bad Name", "run", "boot-smoke"],
        vec!["run", "boot-smoke", "--owner=Bad Name"],
        vec!["--owner", "Bad Name", "exit"],
        vec!["--owner", "Bad Name", "firmware", "list"],
    ] {
        let directory = fixture();
        let root = directory.path();
        let mut command = wrapper(root);
        command.args(args);
        oer_process::Context::default()
            .with(oer_stand_owners::OWNER_KEY, "enclosing-owner")
            .with(oer_stand_arbiter::LEASE_KEY, "enclosing-lease")
            .apply(&mut command)
            .unwrap();
        let output = command.output().unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("`Bad Name` is not an owner name"), "{error}");
        assert!(!root.join("cargo-args").exists(), "runner was built");
        assert!(!root.join("arbiter").exists(), "job was created");
    }
}

#[test]
fn forwards_runner_exit_and_uses_locked_cargo_with_explicit_offline_only() {
    let directory = fixture();
    let root = directory.path();
    for offline in [false, true] {
        let mut command = wrapper(root);
        command.arg("exit").env_remove("CARGO_NET_OFFLINE");
        if offline {
            command.env("CARGO_NET_OFFLINE", "true");
        }
        assert_eq!(command.status().unwrap().code(), Some(37));
        let args = fs::read_to_string(root.join("cargo-args")).unwrap();
        assert!(args.lines().any(|a| a == "--locked"));
        assert!(!args.lines().any(|a| a == "--offline"));
        assert_eq!(
            fs::read_to_string(root.join("offline")).unwrap(),
            if offline { "true" } else { "unset" }
        );
        assert!(root.join("target/hil/current-observer.json").is_file());
    }
}
#[test]
fn cancellation_of_wrapper_pid_reaches_runner_and_waits_for_cleanup_status() {
    let directory = fixture();
    let root = directory.path();
    let mut child = wrapper(root).arg("loop").spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !root.join("pid").is_file() {
        assert!(Instant::now() < deadline, "runner never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("wrapper did not propagate cancellation");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(42));
    assert_eq!(
        fs::read_to_string(root.join("cleaned")).unwrap(),
        "cleaned\n"
    );
}
/// `cargo hil` is this package's binary: the workspace alias runs it and
/// hands it every argument unchanged, with nothing between `--` and them.
#[test]
fn the_cargo_hil_alias_runs_this_binary_with_the_arguments_unchanged() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let config: toml::Table =
        toml::from_str(&fs::read_to_string(root.join(".cargo/config.toml")).unwrap()).unwrap();
    let alias = config["alias"]["hil"].as_str().unwrap();
    let words = alias.split_whitespace().collect::<Vec<_>>();
    let package = words
        .iter()
        .position(|word| *word == "-p")
        .map(|at| words[at + 1]);
    assert_eq!(words.first(), Some(&"run"), "{alias}");
    assert_eq!(package, Some(env!("CARGO_PKG_NAME")), "{alias}");
    assert_eq!(words.last(), Some(&"--"), "{alias}");
}
