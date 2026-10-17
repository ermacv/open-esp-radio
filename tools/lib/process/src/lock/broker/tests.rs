use super::*;

#[test]
fn admission_authenticates_and_each_operation_retains_exclusion() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("device.lock");
    let token = format!(
        "{:_<64}",
        directory.path().file_name().unwrap().to_string_lossy()
    );
    let broker =
        LockBroker::start(FileLock::acquire(&path, Mode::Exclusive).unwrap(), &token).unwrap();
    let mut unauthenticated =
        UnixStream::connect_addr(&address(&std::fs::metadata(&path).unwrap()).unwrap()).unwrap();
    unauthenticated.set_read_timeout(Some(HANDSHAKE)).unwrap();
    unauthenticated.write_all(&[b'b'; REQUEST_BYTES]).unwrap();
    assert!(unauthenticated.read_exact(&mut [0]).is_err());
    let first = LockBroker::operation(&path, &token, broker.pid()).unwrap();
    let second = LockBroker::operation(&path, &token, broker.pid()).unwrap();
    let broker_pid = broker.pid();
    drop(broker);
    assert!(LockBroker::operation(&path, &token, broker_pid).is_err());
    drop(first);
    assert!(
        FileLock::try_acquire(&path, Mode::Exclusive)
            .unwrap()
            .is_none()
    );
    drop(second);
    wait_for_release(&path);
}

fn wait_for_release(path: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while FileLock::try_acquire(path, Mode::Exclusive)
        .unwrap()
        .is_none()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "broker retained completed I/O"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn release_ignores_unrelated_file_description_copies_after_io_drains() {
    let directory = tempfile::tempdir().unwrap();
    for draining in [false, true] {
        let path = directory.path().join(format!("device-{draining}.lock"));
        let token = "a".repeat(TOKEN_BYTES);
        let lock = FileLock::acquire(&path, Mode::Exclusive).unwrap();
        // A concurrent fork can retain this same open file description.
        // Keep a duplicate alive to make close-only release fail reliably.
        let unrelated = lock.file().try_clone().unwrap();
        let broker = LockBroker::start(lock, &token).unwrap();
        let operation =
            draining.then(|| LockBroker::operation(&path, &token, broker.pid()).unwrap());
        drop(broker);
        if operation.is_some() {
            assert!(
                FileLock::try_acquire(&path, Mode::Exclusive)
                    .unwrap()
                    .is_none()
            );
        }
        drop(operation);
        wait_for_release(&path);
        drop(unrelated);
    }
}

#[test]
fn long_lock_paths_and_identical_filenames_have_independent_brokers() {
    let directory = tempfile::tempdir().unwrap();
    let long = directory.path().join("x".repeat(120));
    let first = long.join("first/device.lock");
    let second = long.join("second/device.lock");
    let token = format!(
        "{:_<64}",
        directory.path().file_name().unwrap().to_string_lossy()
    );
    let first_broker =
        LockBroker::start(FileLock::acquire(&first, Mode::Exclusive).unwrap(), &token).unwrap();
    let second_broker =
        LockBroker::start(FileLock::acquire(&second, Mode::Exclusive).unwrap(), &token).unwrap();
    let first_operation = LockBroker::operation(&first, &token, first_broker.pid()).unwrap();
    let second_operation = LockBroker::operation(&second, &token, second_broker.pid()).unwrap();
    let first_pid = first_broker.pid();
    drop(first_broker);
    assert!(LockBroker::operation(&first, &token, first_pid).is_err());
    assert!(LockBroker::operation(&second, &token, second_broker.pid()).is_ok());
    assert!(
        FileLock::try_acquire(&first, Mode::Exclusive)
            .unwrap()
            .is_none()
    );
    drop(first_operation);
    drop(second_operation);
    drop(second_broker);
    wait_for_release(&first);
    wait_for_release(&second);
}

#[test]
fn unexpected_server_credentials_are_rejected_before_sending_the_capability() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("device.lock");
    let lock = FileLock::acquire(&path, Mode::Exclusive).unwrap();
    let listener =
        UnixListener::bind_addr(&address(&lock.file().metadata().unwrap()).unwrap()).unwrap();
    // Real peer credential mismatches without root or a UID change: reject
    // both a foreign UID and a same-UID impostor with the wrong broker PID.
    // SAFETY: geteuid has no arguments or memory preconditions.
    let uid = unsafe { libc::geteuid() };
    for (expected_uid, expected_pid) in [
        (uid.wrapping_add(1), std::process::id()),
        (uid, std::process::id() + 1),
    ] {
        let error = LockBroker::operation_for_peer(
            &path,
            &"s".repeat(TOKEN_BYTES),
            expected_uid,
            expected_pid,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unexpected peer credentials"));
        let (mut connection, _) = listener.accept().unwrap();
        connection.set_read_timeout(Some(HANDSHAKE)).unwrap();
        assert_eq!(connection.read(&mut [0; TOKEN_BYTES]).unwrap(), 0);
    }
}

#[test]
fn native_exit_fences_survive_foreign_completion_and_new_admission() {
    use crate::CommandExt as _;
    use std::os::unix::process::CommandExt as _;

    const ROLE: &str = "OER_BROKER_NATIVE_EXIT_TEST";
    const TEST: &str =
        "lock::broker::tests::native_exit_fences_survive_foreign_completion_and_new_admission";
    let token = "n".repeat(TOKEN_BYTES);
    if let Some(root) = std::env::var_os(ROLE) {
        let root = std::path::PathBuf::from(root);
        let path = root.join("device.lock");
        let mut connection =
            UnixStream::connect_addr(&address(&std::fs::metadata(&path).unwrap()).unwrap())
                .unwrap();
        connection.set_read_timeout(Some(HANDSHAKE)).unwrap();
        let mut request = [0_u8; REQUEST_BYTES];
        request[..TOKEN_BYTES].copy_from_slice(token.as_bytes());
        request[TOKEN_BYTES..]
            .copy_from_slice(&rustix::fs::fstat(&connection).unwrap().st_ino.to_ne_bytes());
        let caller = crate::proc::pidfd_open(std::process::id()).unwrap();
        crate::socket::send(connection.as_raw_fd(), &request, &[caller.as_raw_fd()]).unwrap();
        let mut admitted = [0];
        connection.read_exact(&mut admitted).unwrap();
        assert_eq!(admitted, [1]);

        // An inherited connection must not let a different process declare
        // the native caller's I/O complete. Its kernel credentials differ.
        let mut stranger = std::process::Command::new("true");
        let fd = connection.as_raw_fd();
        // SAFETY: fd stays live through spawn; this callback only sends a
        // fixed socket message after fork, without allocation or locks.
        unsafe {
            stranger.pre_exec(move || crate::socket::send(fd, b"D", &[]));
        }
        assert!(stranger.status().unwrap().success());
        // Simulate unexpected descriptor closure before native I/O completed.
        drop(connection);
        std::fs::write(root.join("closed"), "ready").unwrap();
        std::thread::sleep(Duration::from_secs(30));
        return;
    }

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("device.lock");
    let broker =
        LockBroker::start(FileLock::acquire(&path, Mode::Exclusive).unwrap(), &token).unwrap();
    let _cleanup = BrokerChildCleanup(&broker);
    let mut command = crate::command(std::env::current_exe().unwrap());
    command.args(["--exact", TEST]).env(ROLE, directory.path());
    let mut caller = command.spawn_owned().unwrap();
    let deadline = std::time::Instant::now() + HANDSHAKE;
    while !directory.path().join("closed").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "native fixture did not close its socket"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Admission must not reuse a socket slot still waiting on its caller pidfd.
    let next = LockBroker::operation(&path, &token, broker.pid()).unwrap();
    drop(next);
    (&broker.owner).write_all(&[1]).unwrap();
    broker.owner.shutdown(std::net::Shutdown::Write).unwrap();
    let mut draining = [0];
    (&broker.owner).read_exact(&mut draining).unwrap();
    assert_eq!(
        draining,
        [1],
        "live native caller was released on socket EOF"
    );
    assert!(
        FileLock::try_acquire(&path, Mode::Exclusive)
            .unwrap()
            .is_none()
    );
    // Owner-loss cleanup also terminates an uncompleted native caller whose
    // socket already closed. Only its complete exit permits board release.
    assert!(
        !caller
            .wait_timeout(Some(Duration::from_secs(5)))
            .unwrap()
            .success()
    );
    wait_for_release(&path);
}

/// Fault-inject a syscall error in an isolated fixture and its descendants.
pub(super) fn deny_syscall(syscall: libc::c_long, error: i32) {
    let filter = [
        libc::sock_filter {
            code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 0,
        }, // seccomp_data.nr
        libc::sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
            jt: 0,
            jf: 1,
            k: syscall as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ERRNO | error as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ALLOW,
        },
    ];
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_ptr().cast_mut(),
    };
    // SAFETY: this fixture runs in its own process. prctl reads the correctly
    // sized filter only during this call; it copies it into the kernel. The
    // filter restricts the calling thread and its future descendants only.
    unsafe {
        assert_eq!(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0), 0);
        assert_eq!(
            libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                &raw const program
            ),
            0
        );
    }
}

struct BrokerChildCleanup<'a>(&'a LockBroker);

impl Drop for BrokerChildCleanup<'_> {
    fn drop(&mut self) {
        // SAFETY: the borrowed broker cannot have started its reaper yet. An
        // unreaped child keeps its PID reserved; only kill it if waitpid says
        // it is still running. A failed regression must not leave it spinning.
        unsafe {
            if libc::waitpid(self.0.pid, std::ptr::null_mut(), libc::WNOHANG) == 0 {
                libc::kill(self.0.pid, libc::SIGKILL);
                while libc::waitpid(self.0.pid, std::ptr::null_mut(), 0) < 0
                    && *libc::__errno_location() == libc::EINTR
                {}
            }
        }
    }
}

#[test]
fn persistent_poll_errors_back_off_and_still_drain_operations() {
    const ROLE: &str = "OER_BROKER_POLL_ERROR_TEST";
    if std::env::var_os(ROLE).is_none() {
        let mut command = crate::command(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "lock::broker::tests::persistent_poll_errors_back_off_and_still_drain_operations",
            ])
            .env(ROLE, "fixture");
        crate::capture(&mut command).unwrap();
        return;
    }
    deny_syscall(libc::SYS_poll, libc::ENOMEM);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("device.lock");
    let token = "a".repeat(TOKEN_BYTES);
    let broker =
        LockBroker::start(FileLock::acquire(&path, Mode::Exclusive).unwrap(), &token).unwrap();
    let _cleanup = BrokerChildCleanup(&broker);
    let first = LockBroker::operation(&path, &token, broker.pid()).unwrap();
    let second = LockBroker::operation(&path, &token, broker.pid()).unwrap();
    (&broker.owner).write_all(&[1]).unwrap();
    broker.owner.shutdown(std::net::Shutdown::Write).unwrap();
    let mut owner = &broker.owner;
    let mut draining = [0];
    owner.read_exact(&mut draining).unwrap();
    assert_eq!(draining, [1]);
    assert!(LockBroker::operation(&path, &token, broker.pid()).is_err());
    // Keep the original owner alive through a cleanup scan. Its pinned /proc
    // identity must exempt it while these local operations finish naturally.
    std::thread::sleep(Duration::from_millis(1200));
    drop(first);
    assert!(
        FileLock::try_acquire(&path, Mode::Exclusive)
            .unwrap()
            .is_none()
    );
    drop(second);
    wait_for_release(&path);
    // SAFETY: rusage is a C structure of integer fields, all valid when zero.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: broker.pid is this fixture's live, unreaped child, and wait4
    // writes only the valid local rusage. No reaper starts until broker drops.
    assert_eq!(
        unsafe { libc::wait4(broker.pid, std::ptr::null_mut(), 0, &mut usage) },
        broker.pid
    );
    let cpu_micros = (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) * 1_000_000
        + usage.ru_utime.tv_usec
        + usage.ru_stime.tv_usec;
    assert!(
        cpu_micros < 100_000,
        "poll errors consumed {cpu_micros} us of CPU"
    );
}
