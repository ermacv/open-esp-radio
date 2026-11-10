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
//! the process to stop; drains its stream to the end within a bound (after
//! a cancellation into the sink, so the pcap's tail is kept; otherwise
//! discarding and counting); waits for its exit within a bound; stops the
//! stream's controlled reader (`stream.rs`: it waits in `poll` on its
//! source and a stop pipe, so the stop always ends it) and joins it;
//! removes the interface and reads the router back to confirm it is gone.
//! Each step is attempted whatever the one before returned, and each
//! result is kept. A capture succeeds only when it was cancelled, its
//! stream ended within the bound with nothing discarded, the process
//! exited with success, its reader ended without an error, the interface
//! is confirmed gone and its diagnostics were read whole. Every wait is bounded on an injected
//! [`Clock`]: the service uses the host's, tests drive their own.
//!
//! The pcap bytes go to the [`Sink`] alone; the process's diagnostics and
//! the run's ending go into the [`Report`] or the error, never into the
//! stream. Diagnostics that could not be read whole (a read error, an
//! unbounded or non-UTF-8 stderr, a panic of their reader) fail the
//! capture, with the prefix that was read.

use std::time::Duration;

mod stream;
pub use stream::{Clock, HostClock, Next, Observer, ReaderEvent, STREAM_DEPTH, Stream, Wait};

use serde::Serialize;

use crate::ssh::{Result, Stderr, read_stderr};
use std::{
    io::Read,
    sync::{Arc, Mutex, PoisonError},
    thread::JoinHandle,
};

/// The interface the capture creates: no stand file names it.
pub const INTERFACE: &str = "oercap0";
/// How long `tcpdump` may take to report that it listens.
pub const READY: Duration = Duration::from_secs(10);
/// How long the process may take to exit after the stop request.
pub const EXIT: Duration = Duration::from_secs(10);
/// How long one look at the stream waits before the cancellation is read
/// again.
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

/// The capture process on the router.
pub trait Process {
    /// Wait until it listens on the interface, until `until`.
    fn ready<C: Clock>(&mut self, clock: &C, until: C::Instant) -> Result<()>;
    /// The next bytes of its pcap stream, waiting until `until`.
    fn next<C: Clock>(&mut self, clock: &C, until: C::Instant) -> Next;
    /// Ask it to exit.
    fn stop(&mut self) -> Result<()>;
    /// Wait for its exit until `until`, then close its stream and join the
    /// stream's reader, then confirm the process gone; how it ended. An
    /// exit other than success, a reader that failed or did not end, and a
    /// process still running are errors.
    fn wait_exit<C: Clock>(&mut self, clock: &C, until: C::Instant) -> Result<String>;
    /// Its diagnostics, bounded; complete only after `wait_exit`.
    fn diagnostics(&mut self) -> Diagnostics;
}

/// A process's diagnostics: the bounded text read, and why reading it
/// failed, if it did. A failure fails the capture; the text is kept.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Diagnostics {
    pub text: String,
    pub error: Option<String>,
}

/// A process's stderr, read on its own thread by [`read_stderr`]. The
/// lines read so far outlive a panic of that thread.
pub struct StderrReader {
    thread: JoinHandle<Stderr>,
    seen: Arc<Mutex<String>>,
}

impl StderrReader {
    /// Read `source` on a new thread, handing each line to `line`.
    pub fn spawn(
        source: impl Read + Send + 'static,
        mut line: impl FnMut(&str) + Send + 'static,
    ) -> Self {
        let seen = Arc::new(Mutex::new(String::new()));
        let kept = seen.clone();
        let thread = std::thread::spawn(move || {
            read_stderr(source, |text| {
                kept.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push_str(text);
                line(text);
            })
        });
        Self { thread, seen }
    }

    /// Join the reader once its source ended: its text, and its read error
    /// or its panic as the diagnostics' error.
    pub fn join(self) -> Diagnostics {
        match self.thread.join() {
            Ok(stderr) => Diagnostics {
                text: stderr.text,
                error: stderr.error,
            },
            Err(_) => Diagnostics {
                text: std::mem::take(
                    &mut *self.seen.lock().unwrap_or_else(PoisonError::into_inner),
                ),
                error: Some("the capture's stderr reader panicked".to_owned()),
            },
        }
    }
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
    /// The process listens: from now on the stream is the capture asked
    /// for. Called once, before the stream's first write after the
    /// readiness; never for a capture that did not become ready.
    fn ready(&mut self);
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

/// What the drain after the stop request did.
#[derive(Debug, Default)]
struct Drain {
    /// Bytes taken into the sink after the stop.
    tail: u64,
    /// Bytes read and not written: the capture has a gap.
    discarded: u64,
    /// The stream ended within the bound.
    ended: bool,
    write_error: Option<String>,
}

impl Drain {
    fn clean(&self) -> bool {
        self.ended && self.discarded == 0 && self.write_error.is_none()
    }

    fn describe(&self) -> String {
        format!(
            "{} tail bytes kept, {} discarded, {}{}",
            self.tail,
            self.discarded,
            if self.ended {
                "the stream ended"
            } else {
                "the stream did not end within the bound"
            },
            self.write_error
                .as_ref()
                .map_or_else(String::new, |error| format!(
                    ", the tail's write failed: {error}"
                ))
        )
    }
}

/// Read the stream to its end within `EXIT`: into `sink` while it takes
/// the bytes, discarding and counting otherwise.
fn drain<C: Clock>(
    process: &mut impl Process,
    mut sink: Option<&mut dyn Sink>,
    clock: &C,
) -> Drain {
    let mut drain = Drain::default();
    let deadline = clock.after(clock.now(), EXIT);
    while clock.now() < deadline {
        match process.next(clock, deadline) {
            Next::Bytes(chunk) => {
                let length = chunk.len() as u64;
                let kept = match sink.as_mut() {
                    Some(sink) => match sink.write(&chunk) {
                        Ok(()) => true,
                        Err(error) => {
                            drain.write_error = Some(error);
                            false
                        }
                    },
                    None => false,
                };
                if kept {
                    drain.tail = drain.tail.saturating_add(length);
                } else {
                    sink = None;
                    drain.discarded = drain.discarded.saturating_add(length);
                }
            }
            Next::Idle => {}
            Next::Ended => {
                drain.ended = true;
                break;
            }
        }
    }
    drain
}

/// The teardown's results, each step's own.
struct Teardown {
    stop: Option<Result<()>>,
    drain: Option<Drain>,
    exit: Option<Result<String>>,
    removed: Result<()>,
    diagnostics: Diagnostics,
}

impl Teardown {
    fn clean(&self) -> bool {
        self.stop.as_ref().is_none_or(Result::is_ok)
            && self.drain.as_ref().is_none_or(Drain::clean)
            && self.exit.as_ref().is_none_or(Result::is_ok)
            && self.removed.is_ok()
            && self.diagnostics.error.is_none()
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
            "stop: {}; drain: {}; exit: {exit}; interface removal: {}; diagnostics: {}{}",
            step(&self.stop),
            self.drain
                .as_ref()
                .map_or_else(|| "not started".to_owned(), Drain::describe),
            self.removed
                .as_ref()
                .map_or_else(Clone::clone, |()| "confirmed".to_owned()),
            self.diagnostics.text.trim(),
            self.diagnostics
                .error
                .as_ref()
                .map_or_else(String::new, |error| format!(
                    "; the diagnostics' read failed: {error}"
                ))
        )
    }
}

/// Stop `process` if there is one, drain its stream (into `sink` when
/// given), wait for it, remove the interface and confirm it gone: every
/// step whatever the others returned.
fn teardown<H: Host, C: Clock>(
    host: &mut H,
    process: Option<(&mut H::Process, Option<&mut dyn Sink>)>,
    clock: &C,
) -> Teardown {
    let (stop, drained, exit, diagnostics) = match process {
        None => (None, None, None, Diagnostics::default()),
        Some((process, sink)) => {
            let stop = process.stop();
            let drained = drain(process, sink, clock);
            let exit = process.wait_exit(clock, clock.after(clock.now(), EXIT));
            (Some(stop), Some(drained), Some(exit), process.diagnostics())
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
        drain: drained,
        exit,
        removed,
        diagnostics,
    }
}

/// Capture the radio's `channel` into `sink` until it cancels; every wait
/// is bounded on `clock`.
///
/// # Errors
/// A refused start before anything changed; otherwise the stream's ending
/// together with every teardown step's result.
pub fn capture<H: Host, S: Sink, C: Clock>(
    host: &mut H,
    channel: u8,
    sink: &mut S,
    clock: &C,
) -> Result<Report> {
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
        let teardown = teardown(host, None, clock);
        return Err(format!(
            "the monitor was not created: {error}; {}",
            teardown.describe()
        ));
    }
    let mut process = match host.start(INTERFACE) {
        Ok(process) => process,
        Err(error) => {
            let teardown = teardown(host, None, clock);
            return Err(format!(
                "the capture did not start: {error}; {}",
                teardown.describe()
            ));
        }
    };
    if let Err(error) = process.ready(clock, clock.after(clock.now(), READY)) {
        let teardown = teardown(host, Some((&mut process, None)), clock);
        return Err(format!(
            "the capture did not become ready: {error}; {}",
            teardown.describe()
        ));
    }
    sink.ready();
    let mut bytes = 0u64;
    let ended = loop {
        if sink.cancelled() {
            break Ended::Cancelled;
        }
        match process.next(clock, clock.after(clock.now(), POLL)) {
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
    // After a cancellation the tail goes on into the sink; after a write
    // error or the process's own end there is nothing to keep it for.
    let keep: Option<&mut dyn Sink> = match ended {
        Ended::Cancelled => Some(sink),
        Ended::WriteError(_) | Ended::ProcessEnded => None,
    };
    let teardown = teardown(host, Some((&mut process, keep)), clock);
    let tail = teardown.drain.as_ref().map_or(0, |drain| drain.tail);
    let bytes = bytes.saturating_add(tail);
    match (&ended, teardown.clean(), &teardown.exit) {
        (Ended::Cancelled, true, Some(Ok(exit))) => Ok(Report {
            interface: INTERFACE.to_owned(),
            channel,
            bytes,
            exit: exit.clone(),
            diagnostics: teardown.diagnostics.text,
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

/// Whether a trimmed stderr line of `tcpdump` says it listens: tcpdump
/// 4.99 writes `tcpdump: listening on <interface>, link-type …` (its
/// program name first); the bare form is accepted too.
fn announces_listening(line: &str) -> bool {
    line.strip_prefix("tcpdump: ")
        .unwrap_or(line)
        .starts_with("listening on ")
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
    use super::{
        Clock, Diagnostics, Host, Listing, Next, Process, StderrReader, Stream, Wait, parse_listing,
    };
    use crate::{
        router::quote,
        ssh::{Remote, Result, Ssh},
    };
    use std::sync::{Arc, Mutex, PoisonError, mpsc};

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

    /// `tcpdump` on the router, its stream read by a [`Stream`].
    pub struct SshProcess {
        ssh: Ssh,
        child: oer_tools_os_process::owned::Child,
        stream: Stream,
        ready: mpsc::Receiver<()>,
        pid: Arc<Mutex<Option<u32>>>,
        stderr: Option<StderrReader>,
        diagnostics: Diagnostics,
    }

    /// One line of the capture's stderr: the shell's PID line names the
    /// process; tcpdump's listening line is its readiness.
    pub(super) fn observe(line: &str, pid: &Mutex<Option<u32>>, announce: &mpsc::SyncSender<()>) {
        let line = line.trim();
        if let Some(number) = line.strip_prefix("oer-capture-pid ") {
            *pid.lock().unwrap_or_else(PoisonError::into_inner) = number.trim().parse().ok();
        } else if super::announces_listening(line) {
            let _ = announce.try_send(());
        }
    }

    impl SshProcess {
        fn start(ssh: Ssh, script: &str) -> Result<Self> {
            let mut child = ssh.start(script)?;
            let stdout = child.take_stdout().ok_or("the capture has no stdout")?;
            let stderr = child.take_stderr().ok_or("the capture has no stderr")?;
            let stream = Stream::spawn(stdout, ())?;
            let pid = Arc::new(Mutex::new(None));
            let (announce, ready) = mpsc::sync_channel(1);
            let seen = pid.clone();
            let stderr = StderrReader::spawn(stderr, move |line| observe(line, &seen, &announce));
            Ok(Self {
                ssh,
                child,
                stream,
                ready,
                pid,
                stderr: Some(stderr),
                diagnostics: Diagnostics::default(),
            })
        }

        fn pid(&self) -> Option<u32> {
            *self.pid.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl Process for SshProcess {
        fn ready<C: Clock>(&mut self, clock: &C, until: C::Instant) -> Result<()> {
            match clock.recv(&self.ready, until) {
                Wait::Message(()) => Ok(()),
                Wait::Timeout | Wait::Disconnected => {
                    Err("tcpdump did not report that it listens".into())
                }
            }
        }

        fn next<C: Clock>(&mut self, clock: &C, until: C::Instant) -> Next {
            self.stream.next(clock, until)
        }

        fn stop(&mut self) -> Result<()> {
            let pid = self.pid().ok_or("the capture never named its process")?;
            self.ssh
                .run(&format!("kill -TERM {pid} 2>/dev/null || true\n"))?;
            Ok(())
        }

        fn wait_exit<C: Clock>(&mut self, clock: &C, until: C::Instant) -> Result<String> {
            let within = clock.remaining(until);
            let waited = self.child.wait_timeout(Some(within));
            if waited.is_err() {
                let _ = self.child.kill();
            }
            // The stream stops its reader whatever the source does.
            let read = self.stream.close(clock, until);
            // Its read error or panic fails the capture, after every step.
            if let Some(stderr) = self.stderr.take() {
                self.diagnostics = stderr.join();
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
            let exit = match waited {
                Ok(status) if status.success() => Ok(format!("exited ({status})")),
                Ok(status) => Err(format!("the capture exited with {status}")),
                Err(error) => Err(format!(
                    "the local side did not end within {} s ({error})",
                    within.as_secs()
                )),
            };
            match (exit, read, gone) {
                (Ok(exit), Ok(bytes), Ok(())) => Ok(format!("{exit}, {bytes} bytes read")),
                (exit, read, gone) => Err(format!(
                    "exit: {}; reader: {}; on the router: {}",
                    exit.unwrap_or_else(|e| e),
                    read.map_or_else(|e| e, |bytes| format!("{bytes} bytes read")),
                    gone.map_or_else(|e| format!("may still run: {e}"), |()| "gone".to_owned())
                )),
            }
        }

        fn diagnostics(&mut self) -> Diagnostics {
            self.diagnostics.clone()
        }
    }
}

#[cfg(test)]
mod tests;
