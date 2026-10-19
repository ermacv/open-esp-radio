//! Cancel the actual installer CLI during its unprivileged build, before sudo.
#![cfg(target_os = "linux")]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Session(Child);

impl Drop for Session {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_some()) {
            return;
        }
        // This child created its own session and process group. Kill only
        // that group while its leader is still owned and has not been reaped.
        if let Some(group) = rustix::process::Pid::from_raw(self.0.id() as i32) {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
        let _ = self.0.wait();
    }
}

#[test]
fn cancellation_during_unprivileged_build_never_reaches_sudo_apply() {
    let directory = tempfile::tempdir().unwrap();
    let cargo = directory.path().join("cargo");
    let started = directory.path().join("build-started");
    let sudo_called = directory.path().join("sudo-called");
    fs::write(
        &cargo,
        format!(
            "#!/bin/sh\n: >'{}'\nwhile :; do /bin/sleep 1; done\n",
            started.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o700)).unwrap();
    let sudo = directory.path().join("sudo");
    fs::write(
        &sudo,
        format!("#!/bin/sh\n: >'{}'\nexit 99\n", sudo_called.display()),
    )
    .unwrap();
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o700)).unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_stand"));
    command
        .args([
            "--root",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../.."),
            "fixture",
            "install",
        ])
        .args(["--provider", "linux-bluetooth"])
        .env("CARGO", cargo)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", directory.path().display()),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The installer refuses to run as root, and rightly: sudo is only for its
    // final apply. Run as root (a container), the test would see that refusal
    // instead of the build it cancels. A user namespace mapping this process's
    // root to an unprivileged operator id keeps the files reachable while the
    // installer sees a non-root operator.
    oer_os_process::child_setup::new_session(&mut command);
    if rustix::process::geteuid().is_root() {
        oer_os_process::child_setup::unprivileged_in_user_namespace(&mut command, 1000);
    }
    let mut session = Session(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    while !started.exists() {
        assert!(Instant::now() < deadline, "installer build did not start");
        thread::sleep(Duration::from_millis(10));
    }
    // The session still owns this live child; signal only that process.
    let leader = rustix::process::Pid::from_raw(session.0.id() as i32).unwrap();
    rustix::process::kill_process(leader, rustix::process::Signal::TERM).unwrap();
    let status = loop {
        if let Some(status) = session.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "cancelled installer did not exit"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(130));
    assert!(!sudo_called.exists());
}
