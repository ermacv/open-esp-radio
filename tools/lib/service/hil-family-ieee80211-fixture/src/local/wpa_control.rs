//! Shared event-driven control transport for hostapd and wpa_supplicant.

use crate::Result;
use mio::{Events, Interest, Poll, Token, Waker, unix::SourceFd};
use std::{
    os::{fd::AsRawFd, unix::net::UnixDatagram},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

pub struct Control {
    socket: UnixDatagram,
    poll: Poll,
    deadline: Instant,
    pending_event: bool,
    scan_completed: bool,
    trace: Option<std::fs::File>,
    trace_records: usize,
    started: Instant,
    pub last_status: String,
    pub last_event: String,
    pub last_failure_event: Option<String>,
    _notification: oer_process::CancellationNotification,
    _directory: tempfile::TempDir,
}

impl Control {
    pub fn connect(path: &Path) -> Result<Self> {
        #[cfg(target_os = "linux")]
        wait_socket(path, Duration::from_secs(20))?;
        let directory = tempfile::tempdir()?;
        let socket = UnixDatagram::bind(directory.path().join("control"))?;
        socket.connect(path)?;
        socket.set_nonblocking(true)?;
        let poll = Poll::new()?;
        poll.registry().register(
            &mut SourceFd(&socket.as_raw_fd()),
            Token(0),
            Interest::READABLE,
        )?;
        let wake = Arc::new(Waker::new(poll.registry(), Token(1))?);
        let notification = oer_process::notify_on_cancel(move || {
            let _ = wake.wake();
        });
        Ok(Self {
            socket,
            poll,
            deadline: Instant::now() + Duration::from_secs(20),
            pending_event: false,
            scan_completed: false,
            trace: None,
            trace_records: 0,
            started: Instant::now(),
            last_status: String::new(),
            last_event: String::new(),
            last_failure_event: None,
            _notification: notification,
            _directory: directory,
        })
    }

    pub fn record_to(&mut self, file: std::fs::File) {
        self.trace = Some(file);
    }

    fn record(&mut self, kind: &str, message: &str) -> Result<()> {
        use std::io::Write as _;
        if let Some(file) = self.trace.as_mut() {
            if self.trace_records == 4096 {
                return Err("WPA control trace exceeded 4096 records before readiness".into());
            }
            self.trace_records += 1;
            serde_json::to_writer(
                &mut *file,
                &serde_json::json!({
                    "elapsed_micros": self.started.elapsed().as_micros() as u64,
                    "unix_micros": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_micros() as u64,
                    "kind": kind, "message": message,
                }),
            )?;
            file.write_all(b"\n")?;
            file.flush()?;
        }
        Ok(())
    }

    fn observe_event(&mut self, event: &str) -> Result<()> {
        self.record("event", event)?;
        self.last_event = event.trim().to_owned();
        self.scan_completed |= event.contains("CTRL-EVENT-SCAN-RESULTS");
        if event.contains("CTRL-EVENT-AUTH-REJECT")
            || event.contains("CTRL-EVENT-ASSOC-REJECT")
            || event.contains("CTRL-EVENT-SSID-TEMP-DISABLED")
            || event.contains("CTRL-EVENT-DISCONNECTED")
        {
            self.last_failure_event = Some(self.last_event.clone());
        }
        require_running(event)
    }

    fn receive(&mut self) -> Result<String> {
        loop {
            oer_process::check_cancelled()?;
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("WPA control readiness deadline expired".into());
            }
            let mut bytes = [0; 8192];
            match self.socket.recv(&mut bytes) {
                Ok(length) if length < bytes.len() => {
                    return Ok(std::str::from_utf8(&bytes[..length])?.to_owned());
                }
                Ok(_) => {
                    return Err("WPA control response exceeded the control-message limit".into());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            if let Err(error) = self
                .poll
                .poll(&mut Events::with_capacity(2), Some(remaining))
                && error.kind() != std::io::ErrorKind::Interrupted
            {
                return Err(error.into());
            }
        }
    }

    pub fn request(&mut self, command: &str) -> Result<String> {
        self.socket.send(command.as_bytes())?;
        loop {
            let response = self.receive()?;
            if !response.starts_with('<') {
                self.record(command, &response)?;
                if command == "STATUS" {
                    self.last_status = response.clone();
                }
                return Ok(response);
            }
            self.observe_event(&response)?;
            self.pending_event = true;
        }
    }

    pub fn wait_enabled(&mut self) -> Result<String> {
        if self.request("ATTACH")?.trim() != "OK" {
            return Err("hostapd rejected event subscription".into());
        }
        loop {
            // Subscribe first, then inspect: readiness cannot fall between the
            // status read and subscription. Further reads follow actual events.
            let status = self.request("STATUS")?;
            if field(&status, "state") == Some("ENABLED") {
                return Ok(status);
            }
            if std::mem::take(&mut self.pending_event) {
                continue;
            }
            let event = self.receive()?;
            self.observe_event(&event)?;
        }
    }
    /// The helper leaves network zero disabled until this subscription exists.
    pub fn wait_connected(&mut self) -> Result<String> {
        if self.request("ATTACH")?.trim() != "OK" {
            return Err("wpa_supplicant rejected event subscription".into());
        }
        if self.request("ENABLE_NETWORK 0")?.trim() != "OK" {
            return Err("wpa_supplicant rejected enabling the prepared network".into());
        }
        loop {
            let status = self.request("STATUS")?;
            let state = field(&status, "wpa_state").ok_or("supplicant status omitted wpa_state")?;
            // Snapshot discovery on actual scan completion, never on a timer.
            // This is bounded to the control-message limit; truncation is an error.
            if std::mem::take(&mut self.scan_completed) {
                self.request("SCAN_RESULTS")?;
            }
            if state == "COMPLETED" {
                return Ok(status);
            }
            if std::mem::take(&mut self.pending_event) {
                continue;
            }
            let event = self.receive()?;
            self.observe_event(&event)?;
        }
    }
}

/// Observe creation before checking existence, so a fast hostapd cannot race
/// the subscription. Only the startup watchdog is time based.
#[cfg(target_os = "linux")]
fn wait_socket(path: &Path, timeout: Duration) -> Result<()> {
    use rustix::fs::inotify::{self, CreateFlags, WatchFlags};

    let parent = path.parent().ok_or("control socket has no parent")?;
    let fd = inotify::init(CreateFlags::NONBLOCK | CreateFlags::CLOEXEC)?;
    inotify::add_watch(&fd, parent, WatchFlags::CREATE | WatchFlags::MOVED_TO)?;
    let mut poll = Poll::new()?;
    poll.registry()
        .register(&mut SourceFd(&fd.as_raw_fd()), Token(0), Interest::READABLE)?;
    let wake = Arc::new(Waker::new(poll.registry(), Token(1))?);
    let _notification = oer_process::notify_on_cancel(move || {
        let _ = wake.wake();
    });
    let deadline = Instant::now() + timeout;
    loop {
        oer_process::check_cancelled()?;
        if path.try_exists()? {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("WPA daemon did not create its control socket before its deadline".into());
        }
        poll.poll(&mut Events::with_capacity(2), Some(remaining))
            .or_else(|error| {
                if error.kind() == std::io::ErrorKind::Interrupted {
                    Ok(())
                } else {
                    Err(error)
                }
            })?;
        // Event contents are irrelevant; readiness only triggers a new path check.
        let mut events = [0_u8; 4096];
        match rustix::io::read(&fd, &mut events) {
            Ok(_) | Err(rustix::io::Errno::WOULDBLOCK | rustix::io::Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

fn require_running(event: &str) -> Result<()> {
    if event.contains("AP-DISABLED") || event.contains("CTRL-EVENT-TERMINATING") {
        return Err("WPA daemon stopped before readiness".into());
    }
    Ok(())
}

impl Drop for Control {
    fn drop(&mut self) {
        let _ = self.socket.send(b"DETACH");
    }
}

pub fn field<'a>(response: &'a str, key: &str) -> Option<&'a str> {
    response
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
}

#[cfg(test)]
mod tests;
