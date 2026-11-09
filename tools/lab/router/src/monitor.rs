//! A monitor capture on the router's radio, owned whole by its run: the
//! monitor interface it creates and the `tcpdump` process it starts.
//!
//! The capture never takes over what it did not create: an interface of
//! its name anywhere on the router, or any interface on the radio's PHY,
//! refuses the start before anything is changed. Once the interface is
//! created every ending goes through one teardown, whatever ended the run:
//! a partial start (the interface, the process or its readiness refused),
//! a cancellation (which a client's disconnect is, to the service), a
//! write error of the stream, or the process's own end. The teardown asks
//! the process to stop, waits for its exit within a bound, removes the
//! interface and reads the router back to confirm it is gone; each step is
//! attempted whatever the one before returned, and each result is kept.
//!
//! The pcap bytes go to the [`Sink`] alone; the process's diagnostics and
//! the run's ending go into the [`Report`] or the error, never into the
//! stream.

use std::time::Duration;

use serde::Serialize;

use crate::ssh::Result;

/// The interface the capture creates: no stand file names it.
pub const INTERFACE: &str = "oercap0";
/// How long `tcpdump` may take to report that it listens.
pub const READY: Duration = Duration::from_secs(10);
/// How long the process may take to exit after the stop request.
pub const EXIT: Duration = Duration::from_secs(10);
/// How long one look at the stream waits before the cancellation is read.
pub const POLL: Duration = Duration::from_millis(100);

/// The router's interfaces as the capture needs them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Listing {
    /// The radio's PHY (`phyN`).
    pub phy: String,
    /// The interfaces on the radio's PHY.
    pub on_radio: Vec<String>,
    /// Every interface on the router.
    pub all: Vec<String>,
}

/// What the stream held at one look.
#[derive(Debug, Eq, PartialEq)]
pub enum Next {
    Bytes(Vec<u8>),
    /// Nothing within the wait.
    Idle,
    /// The process's stream ended.
    Ended,
}

/// The capture process on the router.
pub trait Process {
    /// Wait until it listens on the interface.
    fn ready(&mut self, within: Duration) -> Result<()>;
    /// The next bytes of its pcap stream, waiting up to `wait`.
    fn next(&mut self, wait: Duration) -> Next;
    /// Ask it to exit.
    fn stop(&mut self) -> Result<()>;
    /// Wait for its exit, then confirm it is gone; how it ended.
    fn wait_exit(&mut self, within: Duration) -> Result<String>;
    /// Its diagnostics, bounded; complete only after `wait_exit`.
    fn diagnostics(&mut self) -> String;
}

/// The router side of a capture.
pub trait Host {
    type Process: Process;
    fn listing(&mut self) -> Result<Listing>;
    /// Create `name` as a monitor on `phy`, up, on `channel`.
    fn create(&mut self, phy: &str, name: &str, channel: u8) -> Result<()>;
    /// Start capturing `name` as pcap with radiotap headers.
    fn start(&mut self, name: &str) -> Result<Self::Process>;
    fn remove(&mut self, name: &str) -> Result<()>;
}

/// Where the pcap stream goes, and whether its reader cancelled.
pub trait Sink {
    /// Take all of `bytes`; an error stops the capture.
    fn write(&mut self, bytes: &[u8]) -> Result<()>;
    fn cancelled(&self) -> bool;
}

/// Why the stream ended.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ended {
    Cancelled,
    WriteError(String),
    ProcessEnded,
}

/// A capture that ended as asked and left nothing behind.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Report {
    pub interface: String,
    pub channel: u8,
    pub bytes: u64,
    pub exit: String,
    pub diagnostics: String,
}

/// The teardown's results, each step's own.
struct Teardown {
    stop: Option<Result<()>>,
    exit: Option<Result<String>>,
    removed: Result<()>,
    diagnostics: String,
}

impl Teardown {
    fn clean(&self) -> bool {
        self.stop.as_ref().is_none_or(Result::is_ok)
            && self.exit.as_ref().is_none_or(Result::is_ok)
            && self.removed.is_ok()
    }

    fn describe(&self) -> String {
        let step = |result: &Option<Result<()>>| match result {
            None => "not started".to_owned(),
            Some(Ok(())) => "done".to_owned(),
            Some(Err(error)) => error.clone(),
        };
        let exit = match &self.exit {
            None => "not started".to_owned(),
            Some(Ok(exit)) => exit.clone(),
            Some(Err(error)) => error.clone(),
        };
        format!(
            "stop: {}; exit: {exit}; interface removal: {}; diagnostics: {}",
            step(&self.stop),
            self.removed
                .as_ref()
                .map_or_else(Clone::clone, |()| "confirmed".to_owned()),
            self.diagnostics.trim()
        )
    }
}

/// Stop `process` if there is one, wait for it, remove the interface and
/// confirm it gone: every step whatever the others returned.
fn teardown<H: Host>(host: &mut H, process: Option<&mut H::Process>) -> Teardown {
    let (stop, exit, diagnostics) = match process {
        None => (None, None, String::new()),
        Some(process) => {
            let stop = process.stop();
            let exit = process.wait_exit(EXIT);
            (Some(stop), Some(exit), process.diagnostics())
        }
    };
    let removal = host.remove(INTERFACE);
    let confirmed = host.listing().and_then(|listing| {
        if listing.all.iter().any(|name| name == INTERFACE) {
            Err(format!("{INTERFACE} is still on the router"))
        } else {
            Ok(())
        }
    });
    let removed = match (removal, confirmed) {
        (_, Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(error),
        (Err(removal), Err(error)) => Err(format!("{removal}; {error}")),
    };
    Teardown {
        stop,
        exit,
        removed,
        diagnostics,
    }
}

/// Capture the radio's `channel` into `sink` until it cancels.
///
/// # Errors
/// A refused start before anything changed; otherwise the stream's ending
/// together with every teardown step's result.
pub fn capture<H: Host>(host: &mut H, channel: u8, sink: &mut impl Sink) -> Result<Report> {
    let listing = host.listing()?;
    if listing.all.iter().any(|name| name == INTERFACE) {
        return Err(format!(
            "an interface {INTERFACE} already exists on the router; the capture does not take \
             it over"
        ));
    }
    if !listing.on_radio.is_empty() {
        return Err(format!(
            "the radio's PHY already has interfaces ({}); a capture creates its monitor only on \
             an idle radio",
            listing.on_radio.join(", ")
        ));
    }
    // From here the interface is the capture's: it was absent.
    if let Err(error) = host.create(&listing.phy, INTERFACE, channel) {
        let teardown = teardown(host, None);
        return Err(format!(
            "the monitor was not created: {error}; {}",
            teardown.describe()
        ));
    }
    let mut process = match host.start(INTERFACE) {
        Ok(process) => process,
        Err(error) => {
            let teardown = teardown(host, None);
            return Err(format!(
                "the capture did not start: {error}; {}",
                teardown.describe()
            ));
        }
    };
    if let Err(error) = process.ready(READY) {
        let teardown = teardown(host, Some(&mut process));
        return Err(format!(
            "the capture did not become ready: {error}; {}",
            teardown.describe()
        ));
    }
    let mut bytes = 0u64;
    let ended = loop {
        if sink.cancelled() {
            break Ended::Cancelled;
        }
        match process.next(POLL) {
            Next::Bytes(chunk) => {
                if let Err(error) = sink.write(&chunk) {
                    break Ended::WriteError(error);
                }
                bytes = bytes.saturating_add(chunk.len() as u64);
            }
            Next::Idle => {}
            Next::Ended => break Ended::ProcessEnded,
        }
    };
    let teardown = teardown(host, Some(&mut process));
    match (&ended, teardown.clean(), &teardown.exit) {
        (Ended::Cancelled, true, Some(Ok(exit))) => Ok(Report {
            interface: INTERFACE.to_owned(),
            channel,
            bytes,
            exit: exit.clone(),
            diagnostics: teardown.diagnostics,
        }),
        _ => Err(format!(
            "the capture ended: {}; {bytes} bytes; {}",
            match &ended {
                Ended::Cancelled => "cancelled".to_owned(),
                Ended::WriteError(error) => format!("the stream's write failed: {error}"),
                Ended::ProcessEnded => "the capture process ended by itself".to_owned(),
            },
            teardown.describe()
        )),
    }
}

/// Read a listing: the radio's PHY name, its index, then `iw dev`.
///
/// # Errors
/// A listing that does not have that shape.
pub fn parse_listing(text: &str) -> Result<Listing> {
    let mut lines = text.lines();
    let phy = lines
        .next()
        .map(str::trim)
        .filter(|phy| phy.starts_with("phy"))
        .ok_or("the router named no PHY for its radio")?
        .to_owned();
    let index: u32 = lines
        .next()
        .and_then(|index| index.trim().parse().ok())
        .ok_or("the router named no PHY index for its radio")?;
    let mut current = None;
    let mut on_radio = Vec::new();
    let mut all = Vec::new();
    for line in lines.map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(number) = line.strip_prefix("phy#") {
            current = Some(
                number
                    .parse::<u32>()
                    .map_err(|_| format!("`iw dev` printed an unknown PHY `{line}`"))?,
            );
        } else if let Some(name) = line.strip_prefix("Interface ") {
            let name = name.trim().to_owned();
            match current {
                Some(phy) if phy == index => on_radio.push(name.clone()),
                Some(_) => {}
                None => return Err(format!("`iw dev` listed {name} under no PHY")),
            }
            all.push(name);
        } else if current.is_none() {
            return Err(format!("`iw dev` printed `{line}` before any PHY"));
        }
    }
    Ok(Listing { phy, on_radio, all })
}

/// The router over SSH.
pub mod ssh {
    use super::{Host, Listing, Next, Process, parse_listing};
    use crate::{
        router::quote,
        ssh::{Remote, Result, Ssh, Stderr, read_stderr},
    };
    use std::{
        io::Read,
        sync::{Arc, Mutex, PoisonError, mpsc},
        thread::JoinHandle,
        time::Duration,
    };

    /// The router's `radio` (its UCI `wifi-device` section) over `ssh`.
    pub struct SshHost {
        ssh: Ssh,
        radio: String,
    }

    impl SshHost {
        pub fn new(ssh: Ssh, radio: &str) -> Self {
            Self {
                ssh,
                radio: radio.to_owned(),
            }
        }
    }

    impl Host for SshHost {
        type Process = SshProcess;

        fn listing(&mut self) -> Result<Listing> {
            let radio = quote(&self.radio)?;
            parse_listing(&self.ssh.run(&format!(
                "set -e\nphy=$(iwinfo nl80211 phyname {radio})\necho \"$phy\"\n\
                 cat \"/sys/class/ieee80211/$phy/index\"\niw dev\n"
            ))?)
        }

        fn create(&mut self, phy: &str, name: &str, channel: u8) -> Result<()> {
            let (phy, name) = (quote(phy)?, quote(name)?);
            self.ssh.run(&format!(
                "set -e\niw phy {phy} interface add {name} type monitor\n\
                 ip link set {name} up\niw dev {name} set channel {channel}\n"
            ))?;
            Ok(())
        }

        fn start(&mut self, name: &str) -> Result<SshProcess> {
            // The shell prints its PID, which `exec` gives tcpdump: the
            // stop signals that process and no other.
            let script = format!(
                "echo \"oer-capture-pid $$\" >&2\n\
                 exec tcpdump -i {} -y IEEE802_11_RADIO -s 0 -U -w -\n",
                quote(name)?
            );
            SshProcess::start(self.ssh.parallel(), &script)
        }

        fn remove(&mut self, name: &str) -> Result<()> {
            self.ssh.run(&format!("iw dev {} del\n", quote(name)?))?;
            Ok(())
        }
    }

    /// `tcpdump` on the router, its stream read by a thread of its own.
    pub struct SshProcess {
        ssh: Ssh,
        child: oer_tools_os_process::owned::Child,
        chunks: mpsc::Receiver<Vec<u8>>,
        ready: mpsc::Receiver<()>,
        pid: Arc<Mutex<Option<u32>>>,
        stdout: Option<JoinHandle<()>>,
        stderr: Option<JoinHandle<Stderr>>,
        diagnostics: String,
    }

    impl SshProcess {
        fn start(ssh: Ssh, script: &str) -> Result<Self> {
            let mut child = ssh.start(script)?;
            let mut stdout = child.take_stdout().ok_or("the capture has no stdout")?;
            let stderr = child.take_stderr().ok_or("the capture has no stderr")?;
            let (send, chunks) = mpsc::sync_channel(64);
            let stdout = std::thread::spawn(move || {
                let mut buffer = vec![0u8; 64 * 1024];
                loop {
                    match stdout.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => {
                            let chunk = buffer.get(..count).unwrap_or_default().to_vec();
                            if send.send(chunk).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
            let pid = Arc::new(Mutex::new(None));
            let (announce, ready) = mpsc::sync_channel(1);
            let seen = pid.clone();
            let stderr = std::thread::spawn(move || {
                read_stderr(stderr, |line| {
                    let line = line.trim();
                    if let Some(number) = line.strip_prefix("oer-capture-pid ") {
                        *seen.lock().unwrap_or_else(PoisonError::into_inner) =
                            number.trim().parse().ok();
                    } else if line.starts_with("listening on ") {
                        let _ = announce.try_send(());
                    }
                })
            });
            Ok(Self {
                ssh,
                child,
                chunks,
                ready,
                pid,
                stdout: Some(stdout),
                stderr: Some(stderr),
                diagnostics: String::new(),
            })
        }

        fn pid(&self) -> Option<u32> {
            *self.pid.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl Process for SshProcess {
        fn ready(&mut self, within: Duration) -> Result<()> {
            self.ready
                .recv_timeout(within)
                .map_err(|_| format!("tcpdump did not listen within {} s", within.as_secs()))
        }

        fn next(&mut self, wait: Duration) -> Next {
            match self.chunks.recv_timeout(wait) {
                Ok(chunk) => Next::Bytes(chunk),
                Err(mpsc::RecvTimeoutError::Timeout) => Next::Idle,
                Err(mpsc::RecvTimeoutError::Disconnected) => Next::Ended,
            }
        }

        fn stop(&mut self) -> Result<()> {
            let pid = self.pid().ok_or("the capture never named its process")?;
            self.ssh
                .run(&format!("kill -TERM {pid} 2>/dev/null || true\n"))?;
            Ok(())
        }

        fn wait_exit(&mut self, within: Duration) -> Result<String> {
            let waited = self.child.wait_timeout(Some(within));
            if waited.is_err() {
                let _ = self.child.kill();
            }
            if let Some(stdout) = self.stdout.take() {
                let _ = stdout.join();
            }
            if let Some(stderr) = self.stderr.take() {
                self.diagnostics = stderr.join().map_or_else(
                    |_| "the capture's stderr reader failed".to_owned(),
                    |stderr| stderr.text,
                );
            }
            // Gone on the router too, whatever the local side saw.
            let gone = match self.pid() {
                None => Err("the capture never named its process".to_owned()),
                Some(pid) => self
                    .ssh
                    .run(&format!(
                        "if kill -0 {pid} 2>/dev/null; then kill -KILL {pid}; sleep 1; \
                         ! kill -0 {pid} 2>/dev/null; fi\n"
                    ))
                    .map(|_| ()),
            };
            match (waited, gone) {
                (Ok(status), Ok(())) => Ok(format!("exited ({status})")),
                (Err(error), Ok(())) => Err(format!(
                    "the local side did not end within {} s ({error}); the process is gone on \
                     the router",
                    within.as_secs()
                )),
                (_, Err(error)) => Err(format!("the process may still run on the router: {error}")),
            }
        }

        fn diagnostics(&mut self) -> String {
            self.diagnostics.clone()
        }
    }
}

#[cfg(test)]
mod tests;
