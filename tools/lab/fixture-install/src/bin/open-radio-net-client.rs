//! Finite root-owned client profile, DHCP hook and bounded DHCP/ping session.
#![forbid(unsafe_code)]

use oer_os_process::{self as process, owned::Child};
use oer_tool_lab_fixture_install::network_client::{
    Hook, Ipv4, Lease, Ping, Profile, SessionCommand, SessionState,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::MetadataExt,
    path::Path,
    process::{ExitCode, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

type Result<T> = oer_tool_lab_fixture_install::Result<T>;
const STATE: &str = "/run/open-radio-net";
const LEASE: &str = "/run/open-radio-net/client-lease.json";
const PID: &str = "/run/open-radio-net/client-session.json";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("linux-net client operation failed: {error}");
            ExitCode::FAILURE
        }
    }
}
fn run() -> Result<()> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let [operation] = arguments.as_slice() else {
        return Err("station helper takes one finite operation".into());
    };
    if operation == "capabilities" {
        println!("{}", oer_tool_lab_fixture_install::network_client::CONTRACT);
        return Ok(());
    }
    if !rustix::process::geteuid().is_root() {
        return Err("station helper requires its finite root launcher".into());
    }
    let meta = fs::symlink_metadata(STATE)?;
    if !meta.is_dir() || meta.uid() != 0 || meta.gid() != 0 || meta.mode() & 0o777 != 0o700 {
        return Err("unsafe station state directory".into());
    }
    match operation.as_str() {
        "profile" => profile(),
        "dhcp" => session(),
        "stop-dhcp" => stop(),
        "bound" | "renew" | "deconfig" | "leasefail" | "nak" => hook(operation),
        _ => Err("unknown station helper operation".into()),
    }
}
fn now() -> Result<u64> {
    // DHCP hooks and the session share one suspend-aware monotonic epoch.
    Ok(u64::try_from(
        rustix::time::clock_gettime(rustix::time::ClockId::Boottime).tv_sec,
    )?)
}
fn write_private(path: impl AsRef<Path>, bytes: &[u8]) -> Result<()> {
    let mut tmp = tempfile::NamedTempFile::new_in(STATE)?;
    tmp.write_all(bytes)?;
    tmp.persist(path)?;
    Ok(())
}
fn remove(path: impl AsRef<Path>) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
fn ip(arguments: &[&str]) -> Result<()> {
    let output = process::output(
        process::command("/usr/bin/ip").args(arguments),
        Some(Duration::from_secs(5)),
    )?;
    if !output.status.success() {
        return Err("station address operation failed".into());
    }
    Ok(())
}
fn deconfig() -> Result<()> {
    remove(LEASE)?;
    ip(&["-4", "address", "flush", "dev", "wlan0"])
}
fn profile() -> Result<()> {
    let mut input = String::new();
    std::io::stdin().take(2049).read_to_string(&mut input)?;
    let p = Profile::parse(&input)?;
    write_private(
        Path::new(STATE).join("client-pending.conf"),
        p.supplicant()?.as_bytes(),
    )?;
    write_private(
        Path::new(STATE).join("client-pending.json"),
        &serde_json::to_vec(&p.ipv4)?,
    )?;
    print!("{}", p.setup());
    Ok(())
}
fn hook(event: &str) -> Result<()> {
    let value = |name: &str| std::env::var(name).map_err(|_| "missing DHCP option");
    let event = Hook::parse(event, &value("interface")?)?;
    match event {
        Hook::Deconfig | Hook::LeaseFail | Hook::Nak => deconfig(),
        Hook::Bound | Hook::Renew => {
            // All untrusted DHCP fields are validated before any address change.
            let parsed = (|| -> Result<Lease> { Ok(Lease::from_hook_options(now()?, value)?) })();
            let lease = match parsed {
                Ok(l) => l,
                Err(e) => {
                    deconfig()?;
                    return Err(e);
                }
            };
            let previous = match fs::read(LEASE) {
                Ok(bytes) => Some(serde_json::from_slice::<Lease>(&bytes)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            };
            if previous.is_none_or(|old| (old.address, old.prefix) != (lease.address, lease.prefix))
            {
                deconfig()?;
            }
            let cidr = format!("{}/{}", lease.address, lease.prefix);
            ip(&["-4", "address", "replace", &cidr, "dev", "wlan0"])?;
            write_private(LEASE, &serde_json::to_vec(&lease)?)?;
            Ok(())
        }
    }
}
fn lease() -> Result<Lease> {
    let bytes = fs::read(LEASE)?;
    if bytes.len() > 1024 {
        return Err("oversized station lease".into());
    }
    let lease: Lease = serde_json::from_slice(&bytes)?;
    if !lease.active(now()?) {
        return Err("station DHCP lease expired".into());
    }
    Ok(lease)
}
fn emit(value: impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string(&value)?);
    std::io::stdout().flush()?;
    Ok(())
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    pid: u32,
    start_ticks: u64,
}

struct Session {
    child: Child,
    _lock: process::lock::FileLock,
    closed: bool,
}
impl Session {
    fn start() -> Result<Self> {
        let _lock = process::lock::FileLock::try_acquire(
            &Path::new(STATE).join("client-session.lock"),
            process::lock::Mode::Exclusive,
        )?
        .ok_or("another station DHCP session is running")?;
        let config: Ipv4 =
            serde_json::from_slice(&fs::read(Path::new(STATE).join("client-network.json"))?)?;
        if !matches!(config, Ipv4::Dhcp {}) {
            return Err("station profile does not request DHCP".into());
        }
        deconfig()?;
        let exe = std::env::current_exe()?;
        let generation = exe.parent().ok_or("station helper has no generation")?;
        let mut cmd = process::command(generation.join("udhcpc"));
        cmd.args([
            "-f", "-R", "-n", "-t", "5", "-T", "3", "-A", "1", "-i", "wlan0", "-s",
        ])
        .arg(&exe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        let child = Child::spawn_with_shutdown_grace(&mut cmd, Duration::from_secs(5))?;
        let session = Self {
            child,
            _lock,
            closed: false,
        };
        let owner = Owner {
            pid: std::process::id(),
            start_ticks: process::proc::start_ticks(std::process::id())
                .ok_or("cannot identify station session")?,
        };
        write_private(PID, &serde_json::to_vec(&owner)?)?;
        Ok(session)
    }
    fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let child = self.child.kill();
        // BusyBox releases on SIGTERM with -R. Flush even if the hook failed.
        let address = deconfig();
        let marker = remove(PID);
        let disconnect = disconnect();
        child?;
        address?;
        marker?;
        disconnect?;
        Ok(())
    }
}

fn disconnect() -> Result<()> {
    let result = process::output(
        process::command("/usr/bin/wpa_cli").args([
            "-p",
            "/run/open-radio-wpa-control",
            "-i",
            "wlan0",
            "disconnect",
        ]),
        Some(Duration::from_secs(5)),
    )?;
    if !result.status.success() || String::from_utf8_lossy(&result.stdout).trim() != "OK" {
        return Err("station deauthentication failed".into());
    }
    Ok(())
}
impl Drop for Session {
    fn drop(&mut self) {
        if process::cleanup(|| self.close()).is_err() {
            eprintln!("linux-net DHCP cleanup failed");
        }
    }
}

fn session() -> Result<()> {
    let _signals = process::install_signal_handlers()?;
    let mut session = Session::start()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        process::check_cancelled()?;
        if session.child.try_wait()?.is_some() {
            return Err("DHCP client exited before acquiring a lease".into());
        }
        if let Ok(current) = lease() {
            emit(serde_json::json!({"event":"bound","lease":current}))?;
            break;
        }
        if Instant::now() >= deadline {
            return Err("station DHCP acquisition timed out".into());
        }
        process::sleep(Duration::from_millis(100))?;
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut input = BufReader::new(std::io::stdin());
        loop {
            let mut line = String::new();
            let reply = match input.by_ref().take(65).read_line(&mut line) {
                Ok(0) => Ok(None),
                Ok(_) if line.len() <= 64 => SessionCommand::parse(&line).map(Some),
                _ => Err("invalid station command"),
            };
            let done = !matches!(reply, Ok(Some(_)));
            if sender.send(reply).is_err() || done {
                break;
            }
        }
    });
    let started = Instant::now();
    let elapsed = || u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let mut state = SessionState::new(0)?;
    loop {
        process::check_cancelled()?;
        state.remaining(elapsed())?;
        if session.child.try_wait()?.is_some() {
            return Err("DHCP client stopped during station session".into());
        }
        lease()?;
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(Some(SessionCommand::Status))) => {
                state.command(SessionCommand::Status, elapsed())?;
                emit(serde_json::json!({"event":"lease","lease":lease()?}))?
            }
            Ok(Ok(Some(SessionCommand::Ping))) => {
                let remaining = state.command(SessionCommand::Ping, elapsed())?;
                let before = lease()?;
                let output = process::output(
                    process::command("/usr/bin/ping")
                        .args(["-n", "-c", "100", "-i", "1", "-W", "1", "-I", "wlan0"])
                        .arg(before.router.to_string()),
                    Some(Duration::from_micros(remaining.min(115_000_000))),
                )?;
                let text = std::str::from_utf8(&output.stdout)
                    .map_err(|_| "invalid station ping output")?;
                let ping =
                    Ping::parse(text, output.status.code().ok_or("station ping terminated")?)?;
                emit(serde_json::json!({"event":"ping","ping":ping}))?;
                let after = lease()?;
                if (before.address, before.router, before.prefix)
                    != (after.address, after.router, after.prefix)
                {
                    return Err("station DHCP identity changed during ping".into());
                }
            }
            Ok(Ok(Some(SessionCommand::Disconnect))) | Ok(Ok(None)) => {
                state.command(SessionCommand::Disconnect, elapsed())?;
                session.close()?;
                emit(serde_json::json!({"event":"disconnected"}))?;
                return Ok(());
            }
            Ok(Err(e)) => return Err(e.into()),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn stop() -> Result<()> {
    let record = match fs::read(PID) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if let Some(bytes) = record {
        let owner: Owner = serde_json::from_slice(&bytes)?;
        let pid = i32::try_from(owner.pid)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .ok_or("invalid station session process identity")?;
        let fd = match rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()) {
            Ok(fd) => Some(fd),
            Err(e) if e == rustix::io::Errno::SRCH => None,
            Err(e) => return Err(e.into()),
        };
        if let Some(fd) = fd
            && process::proc::start_ticks(owner.pid) == Some(owner.start_ticks)
        {
            if !exited(&fd)? {
                if fs::read_link(format!("/proc/{}/exe", owner.pid)).ok()
                    != Some(std::env::current_exe()?)
                {
                    return Err("station session marker names an unknown live program".into());
                }
                match rustix::process::pidfd_send_signal(&fd, rustix::process::Signal::TERM) {
                    Ok(()) => {}
                    Err(e) if e == rustix::io::Errno::SRCH => {}
                    Err(e) => return Err(e.into()),
                }
            }
            let end = Instant::now() + Duration::from_secs(20);
            while !exited(&fd)? {
                if Instant::now() >= end {
                    return Err("station session did not clean up".into());
                }
                process::sleep(Duration::from_millis(100))?;
            }
        }
    }
    deconfig()?;
    remove(PID)?;
    Ok(())
}

fn exited(fd: &std::os::fd::OwnedFd) -> Result<bool> {
    let mut fds = [rustix::event::PollFd::new(fd, rustix::event::PollFlags::IN)];
    Ok(rustix::event::poll(
        &mut fds,
        Some(&rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        }),
    )? > 0)
}
