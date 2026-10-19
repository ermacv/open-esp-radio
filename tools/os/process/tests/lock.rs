#![cfg(unix)]

use oer_os_process::lock::{FileLock, Mode};
use std::fs::OpenOptions;

// Keep this check in its own test process. Concurrent forks in other tests
// can inherit the descriptor until exec or cleanup, delaying lock release
// beyond the parent's close even though into_file behaves correctly.
#[test]
fn a_handed_over_descriptor_keeps_its_lock() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("file.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let lock = FileLock::try_lock(file, &path, Mode::Shared)
        .unwrap()
        .unwrap();
    let file = lock.into_file();
    let retained = file.try_clone().unwrap();
    assert!(
        FileLock::try_acquire(&path, Mode::Exclusive)
            .unwrap()
            .is_none()
    );
    drop(file);
    assert!(
        FileLock::try_acquire(&path, Mode::Exclusive)
            .unwrap()
            .is_none()
    );
    drop(retained);
    assert!(
        FileLock::try_acquire(&path, Mode::Exclusive)
            .unwrap()
            .is_some()
    );
}

/// The role of this test binary when the test below runs it as a child.
const HOLDER: &str = "OER_TEST_LOCK_HOLDER";

/// A lock is released when its owner drops it, even while another process
/// holds the same open file description: the drop unlocks explicitly rather
/// than relying on the last close. The child is this test binary, which
/// holds an inherited duplicate until its stdin closes.
#[test]
fn dropping_the_owner_releases_the_lock_while_a_child_holds_its_description() {
    use std::io::Read as _;
    if std::env::var_os(HOLDER).is_some() {
        let mut rest = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut rest);
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture.lock");
    let owner = FileLock::try_acquire(&path, Mode::Exclusive)
        .unwrap()
        .unwrap();
    // `try_clone` duplicates the descriptor: the same open file description.
    let shared = owner.file().try_clone().unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "dropping_the_owner_releases_the_lock_while_a_child_holds_its_description",
            "--nocapture",
        ])
        .env(HOLDER, "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null());
    oer_os_process::child_setup::inherit(&mut command, vec![shared.into()]);
    let mut child = command.spawn().unwrap();
    drop(command);

    drop(owner);
    let successor = FileLock::try_acquire(&path, Mode::Exclusive);

    // Release the child before asserting so a regression leaves no process.
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
    assert!(
        matches!(successor, Ok(Some(_))),
        "the owner's drop retained the lock"
    );
}
