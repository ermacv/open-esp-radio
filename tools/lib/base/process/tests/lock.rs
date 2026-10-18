#![cfg(unix)]

use oer_process::lock::{FileLock, Mode};
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
