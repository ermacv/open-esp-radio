//! The line console of a reference peer board, with the full `@` grammar.
//!
//! The stand's ESP-IDF peers (`hil/peers/*`) speak one text protocol on their
//! USB Serial/JTAG console: `@READY protocol=N key=value...` at boot and after
//! `SYNC`, `@OK <command>` or `@ERR <command> <reason>` for every command,
//! and `@<REPORT> fields...` reports at any time. The grammar is
//! `oer-device-peer-line`; this module owns [`PeerConsole`], the one session
//! every peer driver and `cargo hil peer` speak: opening the console without
//! resetting the board, synchronizing with `SYNC`, commands and their
//! answers, queued reports, and the transcript a run keeps. A driver only
//! renders its commands and types its reports.

use std::{
    collections::VecDeque,
    fs,
    io::{ErrorKind, Read, Write},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use oer_device_peer_line::{Line, Report, parse};

use crate::Result;

/// The refusal of a command: the peer answered `@ERR <command> <reason>`.
#[derive(Debug)]
pub struct PeerRejected {
    pub peer: &'static str,
    pub command: String,
    pub reason: String,
}

impl std::fmt::Display for PeerRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} rejected {}: {}",
            self.peer, self.command, self.reason
        )
    }
}

impl std::error::Error for PeerRejected {}

/// Attempts of one synchronization: bytes sent right after the port opens
/// can be lost.
const SYNC_ATTEMPTS: usize = 3;

/// What a peer must answer `SYNC` with.
#[derive(Clone, Copy, Debug)]
pub struct Expected {
    /// The peer's name in errors, such as `IEEE 802.15.4 peer`.
    pub peer: &'static str,
    /// The protocol this host speaks with it.
    pub protocol: u32,
    /// The `stack=` field its `@READY` must carry, when several images share
    /// the board.
    pub stack: Option<&'static str>,
    /// How to restore the image when the board carries another one.
    pub restore: &'static str,
}

/// A session with a reference peer: commands and their answers, and the
/// reports the peer prints, queued until a driver takes them.
pub struct PeerConsole<L> {
    link: L,
    peer: &'static str,
    reports: VecDeque<Report>,
}

impl PeerConsole<RecordingLink<SerialLink>> {
    /// Record every line of the console `link` into `transcript` and take
    /// the running peer over with `SYNC`.
    pub fn open_recorded(
        link: SerialLink,
        transcript: &PeerTranscript,
        expected: Expected,
        timeout: Duration,
    ) -> Result<Self> {
        Self::synchronize(
            RecordingLink::new(link, transcript.clone()),
            expected,
            timeout,
        )
    }
}

impl<L: PeerLink> PeerConsole<L> {
    /// Take the running peer on `link` over: send `SYNC` until it answers
    /// `@READY` as `expected` says. An empty line first ends whatever
    /// partial line the peer holds, and a rejected `SYNC` (its bytes were
    /// lost) is sent again.
    pub fn synchronize(mut link: L, expected: Expected, timeout: Duration) -> Result<Self> {
        let protocol = synchronize(&mut link, timeout, |report| {
            expected
                .stack
                .is_none_or(|stack| report.field("stack") == Some(stack))
        })?;
        match protocol {
            Some(protocol) if protocol == expected.protocol => Ok(Self {
                link,
                peer: expected.peer,
                reports: VecDeque::new(),
            }),
            Some(protocol) => Err(format!(
                "{} speaks protocol {protocol}, this runner {}",
                expected.peer, expected.protocol
            )
            .into()),
            None => Err(format!(
                "{} did not answer SYNC; the board may carry other firmware: {}",
                expected.peer, expected.restore
            )
            .into()),
        }
    }

    /// Send `line` and wait up to `timeout` for the answer to `command`.
    /// Returns the reports named in `answer` that the peer printed before
    /// it; every other report is queued for [`Self::next_report`]. A
    /// refusal is a [`PeerRejected`] error.
    pub fn command(
        &mut self,
        command: &str,
        line: &str,
        timeout: Duration,
        answer: &[&str],
    ) -> Result<Vec<Report>> {
        self.link.send(line)?;
        let deadline = Instant::now() + timeout;
        let mut reports = Vec::new();
        while let Some(received) = self.link.receive(deadline)? {
            match parse(&received) {
                Some(Line::Ok { command: answered }) if answered == command => {
                    return Ok(reports);
                }
                Some(Line::Err {
                    command: answered,
                    reason,
                }) if answered == command => {
                    return Err(PeerRejected {
                        peer: self.peer,
                        command: line.to_owned(),
                        reason,
                    }
                    .into());
                }
                Some(Line::Report(report)) if answer.contains(&report.name.as_str()) => {
                    reports.push(report);
                }
                Some(Line::Report(report)) => self.reports.push_back(report),
                _ => {}
            }
        }
        Err(format!("{} did not answer `{line}`", self.peer).into())
    }

    /// The line transport, for a driver's tests.
    pub fn link(&self) -> &L {
        &self.link
    }

    /// The next report named in `names` before `timeout`, queued ones first,
    /// or `None`. Reports of other names stay queued.
    pub fn next_report(&mut self, timeout: Duration, names: &[&str]) -> Result<Option<Report>> {
        if let Some(index) = self
            .reports
            .iter()
            .position(|report| names.contains(&report.name.as_str()))
        {
            return Ok(self.reports.remove(index));
        }
        let deadline = Instant::now() + timeout;
        while let Some(received) = self.link.receive(deadline)? {
            if let Some(Line::Report(report)) = parse(&received) {
                if names.contains(&report.name.as_str()) {
                    return Ok(Some(report));
                }
                self.reports.push_back(report);
            }
        }
        Ok(None)
    }
}

/// Send `SYNC` until the peer reports ready with a `@READY` that `accept`
/// takes; its protocol, or `None` after every attempt.
fn synchronize<L: PeerLink>(
    link: &mut L,
    timeout: Duration,
    accept: impl Fn(&Report) -> bool,
) -> Result<Option<u32>> {
    for _ in 0..SYNC_ATTEMPTS {
        link.send("")?;
        link.send("SYNC")?;
        let deadline = Instant::now() + timeout;
        while let Some(line) = link.receive(deadline)? {
            match parse(&line) {
                Some(Line::Ready { protocol, report }) if accept(&report) => {
                    return Ok(Some(protocol));
                }
                Some(Line::Err { .. }) => break,
                _ => {}
            }
        }
    }
    Ok(None)
}

/// Whether the peer on `link` answers `SYNC` with an `@READY` line of any
/// image and protocol: a live console. Every stand peer speaks `SYNC`; the
/// wait is bounded by `SYNC_ATTEMPTS` waits of `timeout`.
pub fn answers_sync<L: PeerLink>(link: &mut L, timeout: Duration) -> Result<bool> {
    Ok(synchronize(link, timeout, |_| true)?.is_some())
}

/// Line transport to the peer.
pub trait PeerLink {
    fn send(&mut self, line: &str) -> Result<()>;
    /// The next complete line before `deadline`, or `None` on timeout.
    fn receive(&mut self, deadline: Instant) -> Result<Option<String>>;
}

/// A peer console's serial line, as the stand opened it: reads time out
/// (`ErrorKind::TimedOut`) instead of blocking.
pub trait PeerLine: Read + Write + Send {}

impl<T: Read + Write + Send> PeerLine for T {}

/// The peer's serial console.
pub struct SerialLink {
    path: String,
    port: Box<dyn PeerLine>,
    buffered: Vec<u8>,
}

/// A transport failure of a peer's console, naming the console and what was
/// being done, so a run report says the peer — not the device under test —
/// failed. The I/O error stays the source: the runner classifies it as an
/// infrastructure fault.
#[derive(Debug)]
pub struct PeerConsoleError {
    path: String,
    operation: String,
    source: Box<dyn std::error::Error + Send + Sync + 'static>,
}

impl PeerConsoleError {
    /// `operation` on the console at `path` failed with `source`.
    pub fn new(
        path: &str,
        operation: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            path: path.to_owned(),
            operation: operation.into(),
            source: Box::new(source),
        }
    }
}

impl std::fmt::Display for PeerConsoleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "peer console {}: {} failed: {}",
            self.path, self.operation, self.source
        )
    }
}

impl std::error::Error for PeerConsoleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
    }
}

impl SerialLink {
    /// The console `port` the stand opened at `path`. The stand opens a
    /// peer's console without resetting it and drops what the running image
    /// printed before (`oer_hil_lab::peer_console`): a USB Serial/JTAG reset
    /// of a peer whose IEEE 802.15.4 radio runs can leave it in ROM
    /// download, so the peer is never reset from here.
    pub fn new(path: &Path, port: Box<dyn PeerLine>) -> Self {
        Self {
            path: path.to_string_lossy().into_owned(),
            port,
            buffered: Vec::new(),
        }
    }
}

impl PeerLink for SerialLink {
    fn send(&mut self, line: &str) -> Result<()> {
        let command = line.split(' ').next().unwrap_or(line);
        self.port
            .write_all(line.as_bytes())
            .and_then(|()| self.port.write_all(b"\n"))
            .and_then(|()| self.port.flush())
            .map_err(|error| {
                PeerConsoleError::new(&self.path, format!("write {command}"), error)
            })?;
        Ok(())
    }

    fn receive(&mut self, deadline: Instant) -> Result<Option<String>> {
        loop {
            if let Some(end) = self.buffered.iter().position(|&byte| byte == b'\n') {
                let line: Vec<u8> = self.buffered.drain(..=end).collect();
                return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let mut chunk = [0; 256];
            match self.port.read(&mut chunk) {
                Ok(read) => self.buffered.extend_from_slice(&chunk[..read]),
                Err(error) if error.kind() == ErrorKind::TimedOut => {}
                Err(error) => {
                    return Err(PeerConsoleError::new(&self.path, "read", error).into());
                }
            }
        }
    }
}

/// Every line exchanged with the peer, with its time since the link opened,
/// shared by the link that records it and the workload that saves it.
#[derive(Clone, Debug)]
pub struct PeerTranscript {
    started: Instant,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Default for PeerTranscript {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            lines: Arc::default(),
        }
    }
}

impl PeerTranscript {
    fn record(&self, direction: &str, line: &str) {
        let millis = self.started.elapsed().as_millis();
        let line = line.trim_end_matches(['\r', '\n']);
        if let Ok(mut lines) = self.lines.lock() {
            lines.push(format!("{millis:>8} {direction} {line}"));
        }
    }

    /// The recorded lines: `>` sent to the peer, `<` received from it.
    pub fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .map(|lines| lines.clone())
            .unwrap_or_default()
    }

    /// Write the recorded lines to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = self.lines().join("\n");
        text.push('\n');
        fs::write(path, text)?;
        Ok(())
    }
}

/// A link that records every line it sends and receives.
pub struct RecordingLink<L> {
    link: L,
    transcript: PeerTranscript,
}

impl<L> RecordingLink<L> {
    pub fn new(link: L, transcript: PeerTranscript) -> Self {
        Self { link, transcript }
    }
}

impl<L: PeerLink> PeerLink for RecordingLink<L> {
    fn send(&mut self, line: &str) -> Result<()> {
        self.transcript.record(">", line);
        self.link.send(line)
    }

    fn receive(&mut self, deadline: Instant) -> Result<Option<String>> {
        let line = self.link.receive(deadline)?;
        if let Some(line) = &line {
            self.transcript.record("<", line);
        }
        Ok(line)
    }
}

#[cfg(test)]
mod console_error_tests {
    use super::*;

    #[test]
    fn a_console_failure_names_the_peer_console_and_keeps_the_io_cause() {
        let error = PeerConsoleError::new(
            "/dev/ttyACM0",
            "write CFG",
            std::io::Error::from(ErrorKind::BrokenPipe),
        );
        assert!(
            error
                .to_string()
                .starts_with("peer console /dev/ttyACM0: write CFG failed: "),
            "{error}"
        );
        let source = std::error::Error::source(&error).expect("an I/O cause");
        assert!(source.is::<std::io::Error>());
    }
}

#[cfg(test)]
mod sync_tests {
    use super::*;
    use oer_device_peer_line::{hex, to_hex};

    use std::collections::VecDeque;

    struct Scripted(VecDeque<String>, Vec<String>);

    impl PeerLink for Scripted {
        fn send(&mut self, line: &str) -> Result<()> {
            self.1.push(line.to_owned());
            Ok(())
        }
        fn receive(&mut self, _deadline: Instant) -> Result<Option<String>> {
            Ok(self.0.pop_front())
        }
    }

    fn scripted(lines: &[&str]) -> Scripted {
        Scripted(
            lines.iter().map(|line| format!("{line}\n")).collect(),
            vec![],
        )
    }

    #[test]
    fn the_grammar_types_ready_answers_and_reports() {
        assert_eq!(
            parse("@READY protocol=1 target=chip-b stack=openthread\r\n"),
            Some(Line::Ready {
                protocol: 1,
                report: Report {
                    name: "READY".into(),
                    fields: vec![
                        "protocol=1".into(),
                        "target=chip-b".into(),
                        "stack=openthread".into()
                    ],
                },
            })
        );
        assert_eq!(
            parse("@READY target=chip-b"),
            None,
            "a ready line needs its protocol"
        );
        assert_eq!(
            parse("@OK CFG"),
            Some(Line::Ok {
                command: "CFG".into()
            })
        );
        assert_eq!(parse("@OK"), None);
        assert_eq!(
            parse("@ERR TX status=0x12 now"),
            Some(Line::Err {
                command: "TX".into(),
                reason: "status=0x12 now".into()
            })
        );
        let Some(Line::Report(report)) = parse("@RX 0a0b rssi=-40 lqi=200 pending=1") else {
            panic!("a report");
        };
        assert_eq!(report.name, "RX");
        assert_eq!(report.positional(0), Some("0a0b"));
        assert_eq!(report.parsed::<i8>("rssi"), Some(-40));
        assert_eq!(report.flag("pending"), Some(true));
        assert_eq!(parse("boot noise"), None);
        assert_eq!(parse("@"), None);
        assert_eq!(hex("0a0B"), Some(vec![10, 11]));
        assert_eq!(hex("abc"), None);
        assert_eq!(to_hex(&[0xde, 0x01]), "de01");
    }

    #[test]
    fn a_command_returns_its_answer_reports_and_queues_the_rest() {
        let mut console = PeerConsole::synchronize(
            scripted(&[
                "@READY protocol=1",
                "@RX 00 rssi=1",
                "@BURST sent=3 failed=0",
                "@OK BURST",
                "@ERR CFG invalid",
            ]),
            Expected {
                peer: "test peer",
                protocol: 1,
                stack: None,
                restore: "",
            },
            Duration::from_millis(1),
        )
        .unwrap();
        let answer = console
            .command("BURST", "BURST STOP", Duration::from_millis(1), &["BURST"])
            .unwrap();
        assert_eq!(answer.len(), 1);
        assert_eq!(answer[0].parsed::<u32>("sent"), Some(3));
        let refused = console
            .command("CFG", "CFG 11", Duration::from_millis(1), &[])
            .unwrap_err();
        assert!(refused.is::<PeerRejected>());
        assert_eq!(refused.to_string(), "test peer rejected CFG 11: invalid");
        let queued = console
            .next_report(Duration::from_millis(1), &["RX"])
            .unwrap()
            .unwrap();
        assert_eq!(queued.positional(0), Some("00"));
    }

    #[test]
    fn a_ready_of_another_stack_or_protocol_is_refused() {
        let expected = Expected {
            peer: "Thread peer",
            protocol: 1,
            stack: Some("openthread"),
            restore: "reflash it",
        };
        let other_stack = PeerConsole::synchronize(
            scripted(&["@READY protocol=1 stack=ieee802154"]),
            expected,
            Duration::from_millis(1),
        );
        assert!(
            other_stack
                .err()
                .unwrap()
                .to_string()
                .contains("did not answer SYNC; the board may carry other firmware: reflash it")
        );
        let newer = PeerConsole::synchronize(
            scripted(&["@READY protocol=2 stack=openthread"]),
            expected,
            Duration::from_millis(1),
        );
        assert_eq!(
            newer.err().unwrap().to_string(),
            "Thread peer speaks protocol 2, this runner 1"
        );
    }

    #[test]
    fn any_ready_line_is_a_live_console() {
        let mut link = scripted(&[
            "boot noise",
            "@READY protocol=1 target=chip-b stack=openthread",
        ]);
        assert!(answers_sync(&mut link, Duration::from_millis(1)).unwrap());
        assert!(link.1.contains(&"SYNC".to_owned()));
    }

    #[test]
    fn a_silent_console_is_not_live_after_every_attempt() {
        let mut link = scripted(&[]);
        assert!(!answers_sync(&mut link, Duration::from_millis(1)).unwrap());
        assert_eq!(
            link.1.iter().filter(|line| *line == "SYNC").count(),
            SYNC_ATTEMPTS
        );
    }
}
