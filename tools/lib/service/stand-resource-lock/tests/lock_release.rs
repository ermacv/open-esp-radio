//! A lock file is released when its owner drops it, even while a process
//! forked meanwhile still holds the open file description.
#![cfg(unix)]
#![deny(unsafe_code)]

use std::{io::Write as _, os::fd::AsRawFd as _, os::unix::net::UnixStream};

use oer_stand_resource_lock::ResourceLock;

#[test]
#[allow(
    unsafe_code,
    reason = "the regression depends on fork-inherited descriptors"
)]
fn dropping_owner_releases_lock_while_a_forked_child_retains_the_descriptor() {
    let root = tempfile::tempdir().unwrap();
    let owner = ResourceLock::try_acquire_in(root.path(), "fixture").unwrap();
    let (mut release_child, child_signal) = UnixStream::pair().unwrap();
    let parent_fd = release_child.as_raw_fd();
    let child_fd = child_signal.as_raw_fd();

    // SAFETY: the child executes only async-signal-safe read/close/_exit calls,
    // without Rust allocation, unwinding or destruction after this fork of a
    // multithreaded test process. The parent retains ordinary Rust execution.
    let child = unsafe { libc::fork() };
    assert!(
        child >= 0,
        "fork failed: {}",
        std::io::Error::last_os_error()
    );
    if child == 0 {
        // SAFETY: both socket descriptors were valid at fork. A byte or EOF
        // ends the child; interrupted reads retry without touching Rust state.
        // _exit prevents inherited Rust owners from running their destructors.
        unsafe {
            libc::close(parent_fd);
            let mut byte = 0_u8;
            while libc::read(child_fd, (&mut byte as *mut u8).cast(), 1) < 0 {}
            libc::_exit(0);
        }
    }
    drop(child_signal);

    // The child's fork-inherited descriptor keeps the same open file
    // description alive even though File::open sets close-on-exec.
    drop(owner);
    let successor = ResourceLock::try_acquire_in(root.path(), "fixture");

    // Reap before asserting so a regression cannot leave a waiting child.
    release_child.write_all(&[1]).unwrap();
    let child = rustix::process::Pid::from_raw(child).unwrap();
    let status = loop {
        match rustix::process::waitpid(Some(child), rustix::process::WaitOptions::empty()) {
            Err(rustix::io::Errno::INTR) => continue,
            result => break result.unwrap().unwrap().1,
        }
    };
    assert!(status.exited() && status.exit_status() == Some(0));
    assert!(
        successor.is_ok(),
        "owner drop retained the fixture lock: {:?}",
        successor.as_ref().err()
    );
}
