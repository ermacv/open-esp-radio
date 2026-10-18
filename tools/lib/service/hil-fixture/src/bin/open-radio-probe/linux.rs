use super::model::{self, Config, Report};
use oer_hil_fixture::linux_socket::LinkAddress;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    net::{
        AddressFamily, SendFlags, SocketFlags, SocketType, bind, eth, netdevice::name_to_index,
        send, socket_with,
    },
};
use std::{
    io::Write,
    os::fd::{AsFd, OwnedFd},
    time::Instant,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const MONITOR: &str = "oerprobe0";

fn iw(args: &[&str]) -> Result<String> {
    let output = oer_process::output(
        oer_process::command("/usr/bin/iw").args(args),
        Some(std::time::Duration::from_secs(5)),
    )?;
    if !output.status.success() {
        return Err(format!("iw failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

struct Monitor {
    owned: bool,
}
impl Drop for Monitor {
    fn drop(&mut self) {
        if self.owned {
            oer_process::cleanup(|| {
                let _ = iw(&["dev", MONITOR, "del"]);
            });
        }
    }
}

struct Control<I> {
    input: I,
    cancel: std::os::unix::net::UnixStream,
    _notification: oer_process::CancellationNotification,
}
impl<I: AsFd> Control<I> {
    fn new(input: I) -> Result<Self> {
        let (read, write) = std::os::unix::net::UnixStream::pair()?;
        write.set_nonblocking(true)?;
        let notification = oer_process::notify_on_cancel(move || {
            let _ = (&write).write(&[1]);
        });
        Ok(Self {
            input,
            cancel: read,
            _notification: notification,
        })
    }
    fn wait(&self, deadline: Instant) -> Result<bool> {
        loop {
            oer_process::check_cancelled()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            let timeout = Timespec::try_from(remaining)?;
            let mut fds = [
                PollFd::new(&self.input, PollFlags::IN),
                PollFd::new(&self.cancel, PollFlags::IN),
            ];
            poll(&mut fds, Some(&timeout))?;
            oer_process::check_cancelled()?;
            if !fds[0].revents().is_empty() {
                return Ok(true);
            }
        }
    }
    fn line(&self, deadline: Instant) -> Result<String> {
        let mut bytes = Vec::new();
        loop {
            if !self.wait(deadline)? {
                return Err("probe command timeout".into());
            }
            let mut byte = [0];
            // Read the descriptor directly: buffered stdin could prefetch bytes
            // and hide readiness from poll.
            let count = rustix::io::read(self.input.as_fd(), &mut byte)?;
            if count == 0 {
                return Err("probe controller disconnected".into());
            }
            if byte[0] == b'\n' {
                return Ok(String::from_utf8(bytes)?);
            }
            bytes.push(byte[0]);
            if bytes.len() > 256 {
                return Err("probe command exceeds 256 bytes".into());
            }
        }
    }
}

fn socket() -> Result<OwnedFd> {
    let fd = socket_with(
        AddressFamily::PACKET,
        SocketType::RAW,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        Some(eth::ALL),
    )?;
    bind(&fd, &LinkAddress::new(name_to_index(&fd, MONITOR)?)?)?;
    Ok(fd)
}

pub fn run() -> Result<()> {
    let _signals = oer_process::install_signal_handlers()?;
    let control = Control::new(std::io::stdin())?;
    use std::os::unix::fs::OpenOptionsExt as _;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/run/open-radio-probe.lock")?;
    let _lock = oer_process::lock::FileLock::try_lock(
        lock,
        std::path::Path::new("/run/open-radio-probe.lock"),
        oer_process::lock::Mode::Exclusive,
    )?
    .ok_or("another probe holds /run/open-radio-probe.lock")?;
    let config: Config =
        serde_json::from_str(&control.line(Instant::now() + std::time::Duration::from_secs(10))?)?;
    config.validate()?;
    // The fixture's controlled client is wlan0. An existing monitor or missing
    // association is rejected before acquiring any resources.
    let link = iw(&["dev", "wlan0", "link"])?;
    let bssid_text = link
        .lines()
        .find_map(|s| s.trim().strip_prefix("Connected to "))
        .and_then(|s| s.split_whitespace().next())
        .ok_or("probe source is not associated")?;
    let bytes: Vec<u8> = bssid_text
        .split(':')
        .map(|v| u8::from_str_radix(v, 16))
        .collect::<std::result::Result<_, _>>()?;
    let bssid: [u8; 6] = bytes.try_into().map_err(|_| "invalid BSSID")?;
    let frequency = 2407 + u16::from(config.channel) * 5;
    if !link
        .lines()
        .any(|s| s.trim() == format!("freq: {frequency}"))
        || !link
            .lines()
            .any(|s| s.trim() == format!("SSID: {}", config.ssid))
    {
        return Err("probe source association does not match scenario SSID/channel".into());
    }
    if iw(&["dev"])?
        .lines()
        .any(|line| line.trim() == format!("Interface {MONITOR}"))
    {
        return Err("probe monitor is already owned".into());
    }
    let info = iw(&["dev", "wlan0", "info"])?;
    let phy = info
        .lines()
        .find_map(|s| s.trim().strip_prefix("wiphy "))
        .ok_or("missing source PHY")?;
    let mut monitor = Monitor { owned: true };
    iw(&[
        "phy",
        &format!("phy{phy}"),
        "interface",
        "add",
        MONITOR,
        "type",
        "monitor",
    ])?;
    let status = oer_process::output(
        oer_process::command("/usr/bin/ip").args(["link", "set", MONITOR, "up"]),
        Some(std::time::Duration::from_secs(5)),
    )?
    .status;
    if !status.success() {
        return Err("cannot enable probe monitor".into());
    }
    let fd = socket()?;
    println!("{}", model::READY);
    std::io::stdout().flush()?;
    if control.line(Instant::now() + std::time::Duration::from_secs(30))? != "start" {
        return Err("expected probe Start".into());
    }
    let start = Instant::now();
    let mut report = Report {
        bssid,
        ..Report::default()
    };
    let result = (|| -> Result<()> {
        for index in 0..model::REQUESTS {
            let request = model::request(index).expect("bounded request index");
            let deadline = start + std::time::Duration::from_micros(request.offset_us);
            if control.wait(deadline)? {
                return Err("probe workload cancelled or controller disconnected".into());
            }
            let lateness = Instant::now()
                .saturating_duration_since(deadline)
                .as_micros() as u64;
            report.maximum_lateness_us = report.maximum_lateness_us.max(lateness);
            if lateness > model::MAX_LATENESS_US {
                return Err("probe pacing deadline missed; no catch-up burst permitted".into());
            }
            let frame = super::frame::encode(&config, bssid, request);
            let written = send(&fd, &frame, SendFlags::empty())?;
            if written != frame.len() {
                return Err("partial probe injection".into());
            }
            report.submitted += 1;
        }
        Ok(())
    })();
    if let Err(error) = result {
        report.error = Some(error.to_string());
    }
    drop(fd);
    // Report only after teardown. Failure of explicit removal is evidence,
    // while Drop remains the fallback on every earlier error.
    if let Err(error) = iw(&["dev", MONITOR, "del"]) {
        report.error = Some(format!("probe cleanup failed: {error}"));
    } else {
        monitor.owned = false;
    }
    serde_json::to_writer(std::io::stdout().lock(), &report)?;
    println!();
    report.validate()?;
    Ok(())
}

#[cfg(test)]
mod tests;
