use std::{
    fs,
    io::Write as _,
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use crate::lock::{FileLock, LockBroker, Mode};

const TEST: &str =
    "lock::broker::retention::tests::owner_loss_stops_session_daemons_and_forked_lifetime_holders";
const ROLE: &str = "OER_RETENTION_TEST_ROLE";
const ROOT: &str = "OER_RETENTION_TEST_ROOT";

fn command(root: &Path, role: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", TEST])
        .env(ROOT, root)
        .env(ROLE, role)
        .stdout(Stdio::null());
    command
}

fn wait_until(description: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "{description}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn session(command: &mut Command) {
    // SAFETY: setsid operates on the newly forked child and has no memory
    // preconditions. The child has not been made a process-group leader.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn detach(command: &mut Command) {
    let mut child = command.spawn().unwrap();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

struct Owner(Child);
impl Drop for Owner {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

struct Finish(PathBuf);
impl Drop for Finish {
    fn drop(&mut self) {
        let _ = fs::write(self.0.join("finish-io"), "finish");
    }
}

fn stopped(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
        matches!(
            stat.rsplit_once(") ").unwrap().1.chars().next(),
            Some('Z' | 'X')
        )
    })
}

#[test]
fn owner_loss_stops_session_daemons_and_forked_lifetime_holders() {
    if let Ok(role) = std::env::var(ROLE) {
        let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
        match role.as_str() {
            "owner" => {
                if root.join("opaque").exists() {
                    // Force /proc/fd inspection to fail even under root; the
                    // filter remains local to this fixture and its broker.
                    super::super::tests::deny_syscall(libc::SYS_readlinkat, libc::EACCES);
                }
                let broker = LockBroker::start(
                    FileLock::acquire(&root.join("device.lock"), Mode::Exclusive).unwrap(),
                    &"a".repeat(64),
                )
                .unwrap();
                let operation =
                    LockBroker::operation(&root.join("device.lock"), &"a".repeat(64), broker.pid())
                        .unwrap();
                let mut wrapper = command(&root, "wrapper");
                operation.lifetime().pin(&mut wrapper).unwrap();
                assert!(wrapper.status().unwrap().success());
            }
            "wrapper" => {
                let mut released = command(&root, "released");
                session(&mut released);
                // SAFETY: mark this child's inherited nonstandard descriptors
                // close-on-exec. Its exec-error pipe stays usable until exec.
                unsafe {
                    released.pre_exec(|| {
                        if libc::syscall(
                            libc::SYS_close_range,
                            3_u32,
                            u32::MAX,
                            libc::CLOSE_RANGE_CLOEXEC,
                        ) != 0
                        {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
                detach(&mut released);
                let mut daemon = command(
                    &root,
                    if root.join("opaque").exists() {
                        "hidden"
                    } else {
                        "daemon"
                    },
                );
                session(&mut daemon);
                detach(&mut daemon);
                wait_until("command was not released", || {
                    root.join("finish-command").exists()
                });
            }
            "daemon" | "grandchild" | "unrelated" | "hidden" | "released" => {
                if role == "hidden" {
                    // SAFETY: this fixture changes only its own dumpability.
                    // This makes same-UID /proc/fd inspection unavailable.
                    assert_eq!(unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) }, 0);
                }
                let _signals = crate::install_signal_handlers().unwrap();
                let mut io = fs::File::create(root.join(format!("{role}-io"))).unwrap();
                fs::write(
                    root.join(format!("{role}-pid")),
                    std::process::id().to_string(),
                )
                .unwrap();
                let mut forked = false;
                let deadline = Instant::now() + Duration::from_secs(30);
                while !root.join("finish-io").exists() {
                    assert!(Instant::now() < deadline, "daemon was not stopped");
                    io.write_all(b"still writing\n").unwrap();
                    // Fork after the first cleanup scan: the next scans must
                    // find this new holder, including its separate session.
                    if role == "daemon" && crate::cancellation_requested() && !forked {
                        let mut grandchild = command(&root, "grandchild");
                        session(&mut grandchild);
                        detach(&mut grandchild);
                        forked = true;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            _ => panic!("unknown retention fixture role"),
        }
        return;
    }

    for kill_owner in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let _finish = Finish(root.to_owned());
        let mut unrelated_command = command(root, "unrelated");
        session(&mut unrelated_command);
        let mut unrelated = Owner(unrelated_command.spawn().unwrap());
        let mut owner = Owner(command(root, "owner").spawn().unwrap());
        wait_until("daemon did not begin I/O", || {
            root.join("daemon-pid").exists() && root.join("released-pid").exists()
        });
        assert!(
            FileLock::try_acquire(&root.join("device.lock"), Mode::Exclusive)
                .unwrap()
                .is_none()
        );
        if kill_owner {
            owner.0.kill().unwrap();
        } else {
            fs::write(root.join("finish-command"), "finish").unwrap();
        }
        assert_eq!(owner.0.wait().unwrap().success(), !kill_owner);
        wait_until("broker did not terminate all retained lifetimes", || {
            FileLock::try_acquire(&root.join("device.lock"), Mode::Exclusive)
                .unwrap()
                .is_some()
        });
        for role in ["daemon", "grandchild"] {
            let pid: u32 = fs::read_to_string(root.join(format!("{role}-pid")))
                .unwrap()
                .parse()
                .unwrap();
            wait_until("lifetime holder is still running", || stopped(pid));
            let path = root.join(format!("{role}-io"));
            let before = fs::metadata(&path).unwrap().len();
            std::thread::sleep(Duration::from_millis(30));
            assert_eq!(
                fs::metadata(path).unwrap().len(),
                before,
                "I/O continued after unlock"
            );
        }
        assert!(
            unrelated.0.try_wait().unwrap().is_none(),
            "unrelated daemon was killed"
        );
        let released: u32 = fs::read_to_string(root.join("released-pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            !stopped(released),
            "daemon that closed its lifetime was killed"
        );
        fs::write(root.join("finish-io"), "finish").unwrap();
        assert!(unrelated.0.wait().unwrap().success());
        wait_until("released daemon did not stop", || stopped(released));
    }
}

#[test]
fn unreadable_lifetime_holders_keep_exclusion_until_their_io_closes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let _finish = Finish(root.to_owned());
    fs::write(root.join("opaque"), "opaque").unwrap();
    let mut owner = Owner(command(root, "owner").spawn().unwrap());
    wait_until("hidden holder did not begin I/O", || {
        root.join("hidden-pid").exists()
    });
    fs::write(root.join("finish-command"), "finish").unwrap();
    assert!(owner.0.wait().unwrap().success());
    // Let both cleanup phases run. The unreadable holder keeps writing and
    // exclusion stays held instead of treating an empty scan as I/O closure.
    std::thread::sleep(Duration::from_millis(2500));
    let before = fs::metadata(root.join("hidden-io")).unwrap().len();
    wait_until("hidden holder stopped unexpectedly", || {
        fs::metadata(root.join("hidden-io")).unwrap().len() > before
    });
    assert!(
        FileLock::try_acquire(&root.join("device.lock"), Mode::Exclusive)
            .unwrap()
            .is_none()
    );
    fs::write(root.join("finish-io"), "finish").unwrap();
    wait_until("closed hidden I/O retained exclusion", || {
        FileLock::try_acquire(&root.join("device.lock"), Mode::Exclusive)
            .unwrap()
            .is_some()
    });
    wait_until("released daemon did not start", || {
        fs::read_to_string(root.join("released-pid")).is_ok_and(|pid| pid.parse::<u32>().is_ok())
    });
    let released: u32 = fs::read_to_string(root.join("released-pid"))
        .unwrap()
        .parse()
        .unwrap();
    wait_until("released daemon did not stop", || stopped(released));
}
