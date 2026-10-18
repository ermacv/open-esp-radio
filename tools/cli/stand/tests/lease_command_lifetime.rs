//! External commands need only inherited lifetime descriptors, not broker APIs.
#![cfg(target_os = "linux")]

use std::{
    fs,
    io::Write as _,
    os::unix::fs::PermissionsExt as _,
    os::unix::process::CommandExt as _,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use oer_device_lock::{Busy, DeviceAccess, DeviceId, Holder};
use oer_process::lock::LockBroker;

const TEST: &str = "lease_shutdown_stops_escaped_writers_before_releasing_every_board";
const ROLE: &str = "OER_LEASE_TEST_ROLE";
const DIRECTORY: &str = "OER_LEASE_TEST_DIRECTORY";
const BOARDS: [&str; 2] = ["00:11:22:33:44:66", "00:11:22:33:44:77"];

struct Owner(Child);

struct WriterCleanup(std::path::PathBuf);

impl Drop for WriterCleanup {
    fn drop(&mut self) {
        let _ = fs::write(self.0.join("finish"), "finish");
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn wait_until(description: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "{description}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn fixture(command: &mut Command, directory: &Path) {
    command
        .env(DIRECTORY, directory)
        .env("XDG_RUNTIME_DIR", directory)
        .env("XDG_CACHE_HOME", directory.join("cache"))
        .env("XDG_DATA_HOME", directory.join("data"))
        .env(
            oer_stand_file::paths::STAND_FILE_ENV,
            directory.join("stand.toml"),
        )
        .env(
            oer_stand_file::paths::ARBITER_ENV,
            directory.join("arbiter"),
        )
        .stdout(Stdio::null());
}

fn contender(directory: &Path, role: &str) {
    let mut command = oer_process::command(std::env::current_exe().unwrap());
    fixture(&mut command, directory);
    command.args(["--exact", TEST]).env(ROLE, role);
    assert!(command.status().unwrap().success(), "{role}");
}

#[test]
fn lease_shutdown_stops_escaped_writers_before_releasing_every_board() {
    if let Ok(role) = std::env::var(ROLE) {
        let directory = std::path::PathBuf::from(std::env::var_os(DIRECTORY).unwrap());
        match role.as_str() {
            "wrapper" => {
                // Model a flasher starting an implicit daemon in a new session.
                let mut writer = Command::new(std::env::current_exe().unwrap());
                writer.args(["--exact", TEST]).env(ROLE, "writer");
                // SAFETY: setsid acts on the new child, which is not a group
                // leader yet, and has no memory preconditions.
                unsafe {
                    writer.pre_exec(|| {
                        if libc::setsid() < 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
                let mut writer = writer.spawn().unwrap();
                if std::env::var_os("OER_LEASE_TEST_NORMAL_EXIT").is_some() {
                    thread::spawn(move || {
                        let _ = writer.wait();
                    });
                    wait_until("wrapper was not released", || {
                        directory.join("end-command").exists()
                    });
                } else {
                    assert!(writer.wait().unwrap().success());
                }
            }
            "writer" => {
                // Model a third-party flasher: no Context or broker calls.
                // Catch TERM and continue I/O so cleanup must reach SIGKILL.
                let _signals = oer_process::install_signal_handlers().unwrap();
                let mut io = fs::File::create(directory.join("io")).unwrap();
                fs::write(directory.join("writer-pid"), std::process::id().to_string()).unwrap();
                fs::write(directory.join("started"), "ready").unwrap();
                let deadline = Instant::now() + Duration::from_secs(60);
                while !directory.join("finish").exists() {
                    assert!(Instant::now() < deadline, "writer was not released");
                    io.write_all(b"external flash still running\n").unwrap();
                    thread::sleep(Duration::from_millis(10));
                }
                drop(io);
                fs::write(directory.join("closed"), "closed").unwrap();
            }
            "busy" | "free" => {
                assert_eq!(
                    oer_process::Context::current().unwrap(),
                    &Default::default()
                );
                for mac in BOARDS {
                    let id: DeviceId = mac.parse().unwrap();
                    let acquire = || DeviceAccess::try_acquire(&id, "unrelated contender").unwrap();
                    if role == "busy" {
                        assert!(matches!(acquire(), Err(Busy::Held(_))));
                    } else {
                        wait_until("board retained after external I/O ended", || {
                            acquire().is_ok()
                        });
                    }
                }
            }
            _ => panic!("unknown fixture role"),
        }
        return;
    }

    // Both explicit board claims and a whole-stand claim pin all selected boards.
    for claims in [
        vec!["--board", "one", "--board", "two", "--air", "none"],
        vec!["--stand"],
    ] {
        for kill_owner in [false, true] {
            let directory = tempfile::Builder::new()
                .prefix(&"x".repeat(120))
                .tempdir()
                .unwrap();
            let _writer_cleanup = WriterCleanup(directory.path().to_owned());
            fs::write(
                directory.path().join("stand.toml"),
                "schema = 1\n[stand]\nid = 'test'\nair = 'exclusive'\n\
             [[board]]\nid = 'one'\nusb-serial = '00:11:22:33:44:66'\nchip = 'esp32c5'\n\
             radios = ['ble']\nroles = ['dut']\nreset = ['jtag']\n\
             [[board]]\nid = 'two'\nusb-serial = '00:11:22:33:44:77'\nchip = 'esp32c5'\n\
             radios = ['ble']\nroles = ['dut']\nreset = ['jtag']\n",
            )
            .unwrap();
            fs::set_permissions(
                directory.path().join("stand.toml"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            let mut lease = oer_process::command(env!("CARGO_BIN_EXE_oer-stand"));
            fixture(&mut lease, directory.path());
            lease
                .args([
                    "--root",
                    concat!(env!("CARGO_MANIFEST_DIR"), "/../.."),
                    "--owner",
                    "lifetime-test",
                    "lease",
                ])
                .args(&claims)
                .arg("--")
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", TEST])
                .env(ROLE, "wrapper");
            if !kill_owner {
                lease.env("OER_LEASE_TEST_NORMAL_EXIT", "1");
            }
            let mut owner = Owner(lease.spawn().unwrap());
            wait_until("leased writer did not start", || {
                directory.path().join("started").exists()
            });
            contender(directory.path(), "busy");
            if kill_owner {
                owner.0.kill().unwrap();
            } else {
                fs::write(directory.path().join("end-command"), "finish").unwrap();
            }
            assert_eq!(owner.0.wait().unwrap().success(), !kill_owner);
            let locks = directory.path().join("open-esp-radio/devices");
            wait_until("owner loss did not close broker admission", || {
                BOARDS.iter().all(|mac| {
                    let lock = locks.join(format!("{}.lock", mac.replace(':', "")));
                    let holder: Holder =
                        serde_json::from_reader(fs::File::open(&lock).unwrap()).unwrap();
                    LockBroker::operation(&lock, &holder.token, holder.broker_pid).is_err()
                })
            });
            let pid: u32 = fs::read_to_string(directory.path().join("writer-pid"))
                .unwrap()
                .parse()
                .unwrap();
            wait_until("escaped writer survived lease shutdown", || {
                fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
                    matches!(
                        stat.rsplit_once(") ").unwrap().1.chars().next(),
                        Some('Z' | 'X')
                    )
                })
            });
            contender(directory.path(), "free");
            let before = fs::metadata(directory.path().join("io")).unwrap().len();
            thread::sleep(Duration::from_millis(30));
            assert_eq!(
                fs::metadata(directory.path().join("io")).unwrap().len(),
                before,
                "I/O continued after board release"
            );
        }
    }
}
