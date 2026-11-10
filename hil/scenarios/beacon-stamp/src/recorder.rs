//! A board console recorded continuously: a worker of its own writes every
//! byte of the LAB operation's stream to the console's file as it arrives,
//! flushed, before it hands the stream's complete lines to whoever waits for
//! a marker. A wait that gives up (a timeout, a result before its marker,
//! the console's end) loses no byte: the file already holds everything the
//! stream delivered. A file that refuses a write stops the record: the
//! worker cancels the operation and every wait from then on fails with that
//! write, never with a line the file does not hold.
//!
//! A console opened on a running board holds a prefix of an earlier boot
//! before the boot it was opened for. The waits count only the lines of
//! the boot observed starting in the stream: its start is the ROM's reset
//! line (`oer_tools_lab_model::rom::is_reset_line`). Lines before it are
//! kept in the file and never read as this boot's; a second reset line is
//! another boot, and every wait from then on is refused.
//!
//! Every ending cancels the operation and joins the worker, and keeps four
//! results apart: the file's write, the bytes the service dropped for a
//! slow reader, the stream's own read, and the operation's end. What the
//! worker observed stays known if it fails; what it could not observe is
//! recorded as unknown.

use crate::{
    result::{ProbeResult, parse},
    sniffer::{Cancel, Operation},
};
use oer_tools_lab_client::client::Event;
use std::{
    cell::Cell,
    io::Write,
    sync::{Arc, Mutex, PoisonError, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// The LAB client's end of an operation whose service went away
/// (`oer_tools_lab_client::client`): the stream's read failed, the
/// operation's own end is unknown.
const SERVICE_GONE: &str = "the LAB service closed the connection";

/// One result of a recording: as observed, or not observed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    Ok,
    Failed(String),
    /// Not observed: why it stays unknown.
    Unknown(String),
}

impl Outcome {
    fn record(&self) -> serde_json::Value {
        match self {
            Self::Ok => serde_json::json!({ "ok": true }),
            Self::Failed(error) => serde_json::json!({ "ok": false, "detail": error }),
            Self::Unknown(why) => serde_json::json!({ "ok": null, "detail": why }),
        }
    }
}

/// The operation's end as the service reported it, or not reported.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum End {
    Reported(Result<serde_json::Value, String>),
    Unknown(String),
}

/// What one recorded console left, each result its own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recording {
    /// Bytes the file confirmed taking.
    pub written: u64,
    /// The file's write: a failure stops the record (the bytes after it are
    /// not in it); unknown while a write was under way when the worker
    /// failed.
    pub write: Outcome,
    /// Bytes the service dropped for a slow reader (`Event::Lost`).
    pub lost: u64,
    /// The stream's read: failed when the service went away before the
    /// operation's end or the worker failed.
    pub read: Outcome,
    pub end: End,
    /// The finish's cancel of the operation.
    pub cancel: Outcome,
}

impl Recording {
    fn new() -> Self {
        Self {
            written: 0,
            write: Outcome::Ok,
            lost: 0,
            read: Outcome::Unknown("the stream was not read to its end".to_owned()),
            end: End::Unknown("not reported".to_owned()),
            cancel: Outcome::Ok,
        }
    }

    /// The recording as the run's outcome records it.
    pub fn record(&self, file: &str) -> serde_json::Value {
        serde_json::json!({
            "file": file,
            "written": self.written,
            "write": self.write.record(),
            "lost": self.lost,
            "read": self.read.record(),
            "cancel": self.cancel.record(),
            "end": match &self.end {
                End::Unknown(why) => serde_json::json!({ "ok": null, "detail": why }),
                End::Reported(Ok(value)) => serde_json::json!({ "ok": true, "detail": value }),
                End::Reported(Err(error)) => serde_json::json!({ "ok": false, "detail": error }),
            },
        })
    }

    /// Whether nothing of the console is missing or unknown and its
    /// operation ended as asked; otherwise every cause, apart.
    pub fn clean(&self) -> Result<(), String> {
        let mut causes = Vec::new();
        for (name, outcome) in [
            ("write", &self.write),
            ("read", &self.read),
            ("cancel", &self.cancel),
        ] {
            match outcome {
                Outcome::Ok => {}
                Outcome::Failed(error) => causes.push(format!("{name}: {error}")),
                Outcome::Unknown(why) => causes.push(format!("{name}: unknown, {why}")),
            }
        }
        if self.lost > 0 {
            causes.push(format!("lost: {} bytes", self.lost));
        }
        match &self.end {
            End::Reported(Ok(_)) => {}
            End::Reported(Err(error)) => causes.push(format!("end: {error}")),
            End::Unknown(why) => causes.push(format!("end: unknown, {why}")),
        }
        if causes.is_empty() {
            Ok(())
        } else {
            Err(causes.join("; "))
        }
    }
}

/// What the worker hands the waits.
enum Message {
    Line(String),
    /// The file refused a write: the record stops here.
    Failed(String),
}

/// Where the waits are in the stream's boots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Boot {
    /// No reset line yet: every line is an earlier boot's prefix.
    Before,
    /// After the first reset line: the current boot's lines.
    Current,
    /// A second reset line came: another boot.
    Again,
}

pub struct Recorder<C: Cancel + Send + 'static> {
    /// The image whose result line ends a result wait.
    image: &'static str,
    lines: mpsc::Receiver<Message>,
    boot: Cell<Boot>,
    cancel: C,
    observed: Arc<Mutex<Recording>>,
    worker: Option<JoinHandle<()>>,
}

fn observe(observed: &Mutex<Recording>, update: impl FnOnce(&mut Recording)) {
    update(&mut observed.lock().unwrap_or_else(PoisonError::into_inner));
}

impl<C: Cancel + Send + 'static> Recorder<C> {
    /// Record `operation`'s stream into `file` from its first byte.
    pub fn start<O: Operation<Cancel = C>>(
        image: &'static str,
        mut operation: O,
        mut file: impl Write + Send + 'static,
    ) -> Self {
        let cancel = operation.canceller();
        let (send, lines) = mpsc::channel();
        let observed = Arc::new(Mutex::new(Recording::new()));
        let recording = observed.clone();
        let worker = std::thread::spawn(move || {
            let mut line = Vec::new();
            let mut recorded = true;
            loop {
                match operation.event() {
                    Event::Output(bytes) => {
                        if !recorded {
                            continue;
                        }
                        let length = bytes.len() as u64;
                        observe(&recording, |recording| {
                            recording.write = Outcome::Unknown(format!(
                                "the recorder stopped during a write of {length} bytes"
                            ));
                        });
                        let written = file.write_all(&bytes).and_then(|()| file.flush());
                        match written {
                            Ok(()) => observe(&recording, |recording| {
                                recording.write = Outcome::Ok;
                                recording.written = recording.written.saturating_add(length);
                            }),
                            Err(error) => {
                                let error = format!("the console file: {error}");
                                observe(&recording, |recording| {
                                    recording.write = Outcome::Failed(error.clone());
                                });
                                // The record has a gap from here: stop it,
                                // and every wait hears why.
                                recorded = false;
                                let _ = operation.cancel();
                                let _ = send.send(Message::Failed(error));
                                continue;
                            }
                        }
                        // In the file first, then to the waits.
                        for byte in bytes {
                            if byte == b'\n' {
                                let text = String::from_utf8_lossy(&line).trim_end().to_owned();
                                let _ = send.send(Message::Line(text));
                                line.clear();
                            } else {
                                line.push(byte);
                            }
                        }
                    }
                    Event::Lost(bytes) => observe(&recording, |recording| {
                        recording.lost = recording.lost.saturating_add(bytes);
                    }),
                    Event::Ready => {}
                    Event::Finished(result) => {
                        if recorded && !line.is_empty() {
                            let text = String::from_utf8_lossy(&line).trim_end().to_owned();
                            let _ = send.send(Message::Line(text));
                        }
                        observe(&recording, |recording| match result {
                            Err(error) if error == SERVICE_GONE => {
                                recording.read = Outcome::Failed(error);
                                recording.end = End::Unknown(
                                    "the service went away before the operation's end".to_owned(),
                                );
                            }
                            result => {
                                recording.read = Outcome::Ok;
                                recording.end = End::Reported(result);
                            }
                        });
                        return;
                    }
                }
            }
        });
        Self {
            image,
            lines,
            boot: Cell::new(Boot::Before),
            cancel,
            observed,
            worker: Some(worker),
        }
    }

    /// The next line of the current boot within `deadline` (none: no
    /// deadline), or why none came: a failed file, the console's end, the
    /// timeout, or another boot.
    fn line(&self, deadline: Option<Instant>, waiting: &str) -> Result<String, String> {
        let ended = || format!("the console ended before {waiting}");
        loop {
            if self.boot.get() == Boot::Again {
                return Err(format!("the board booted again before {waiting}"));
            }
            let message = match deadline {
                None => self.lines.recv().map_err(|_| ended())?,
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    self.lines.recv_timeout(left).map_err(|error| match error {
                        mpsc::RecvTimeoutError::Timeout => {
                            format!("no {waiting} within the console timeout")
                        }
                        mpsc::RecvTimeoutError::Disconnected => ended(),
                    })?
                }
            };
            let line = match message {
                Message::Failed(error) => {
                    return Err(format!(
                        "the console record stopped before {waiting}: {error}"
                    ));
                }
                Message::Line(line) => line,
            };
            if oer_tools_lab_model::rom::is_reset_line(line.trim()) {
                self.boot.set(match self.boot.get() {
                    Boot::Before => Boot::Current,
                    Boot::Current | Boot::Again => Boot::Again,
                });
                continue;
            }
            if self.boot.get() == Boot::Current {
                return Ok(line);
            }
            // An earlier boot's prefix: kept in the file, never this boot's.
        }
    }

    /// Wait until a line of the current boot holds `marker`, within
    /// `timeout`; a result line before it is an error naming that result.
    pub fn wait_for(&self, marker: &str, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now().checked_add(timeout);
        let waiting = format!("`{marker}` line");
        loop {
            let line = self.line(deadline, &waiting)?;
            if line.contains(marker) {
                return Ok(());
            }
            if let Some(result) = parse(&line, self.image)? {
                return Err(format!(
                    "{} ended before `{marker}`: {result:?}",
                    self.image
                ));
            }
        }
    }

    /// Wait for the current boot's result line within `timeout`.
    pub fn result(&self, timeout: Duration) -> Result<ProbeResult, String> {
        let deadline = Instant::now().checked_add(timeout);
        let waiting = format!("{} result line", self.image);
        loop {
            let line = self.line(deadline, &waiting)?;
            if let Some(result) = parse(&line, self.image)? {
                return Ok(result);
            }
        }
    }

    /// Cancel the operation and join the worker: the recording's results.
    pub fn finish(mut self) -> Recording {
        self.end()
    }

    fn end(&mut self) -> Recording {
        let cancelled = self.cancel.cancel();
        let joined = match self.worker.take() {
            None => Err("the recording was already finished".to_owned()),
            Some(worker) => worker
                .join()
                .map_err(|_| "the console recorder failed".to_owned()),
        };
        let mut recording = self
            .observed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Err(error) = joined {
            // The worker failed: its read did, whatever it had seen.
            recording.read = Outcome::Failed(match &recording.read {
                Outcome::Failed(read) => format!("{read}; {error}"),
                Outcome::Ok | Outcome::Unknown(_) => error,
            });
        }
        if let Err(error) = cancelled {
            recording.cancel = Outcome::Failed(error);
        }
        recording
    }
}

impl<C: Cancel + Send + 'static> Drop for Recorder<C> {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.end();
        }
    }
}

/// The run's consoles, each by its name and file: those still recording
/// and those finished. Every one still recording is finished by
/// [`Consoles::finish_all`], so every console's results are kept whatever
/// ended the run.
pub struct Consoles<C: Cancel + Send + 'static> {
    recording: Vec<(&'static str, String, Recorder<C>)>,
    finished: Vec<(&'static str, String, Recording)>,
}

impl<C: Cancel + Send + 'static> Consoles<C> {
    pub const fn new() -> Self {
        Self {
            recording: Vec::new(),
            finished: Vec::new(),
        }
    }

    /// Keep `recorder`, recording into `file`, as `name`.
    pub fn add(&mut self, name: &'static str, file: String, recorder: Recorder<C>) {
        self.recording.push((name, file, recorder));
    }

    /// The console `name` still recording.
    pub fn get(&self, name: &str) -> Result<&Recorder<C>, String> {
        self.recording
            .iter()
            .find(|(held, _, _)| *held == name)
            .map(|(_, _, recorder)| recorder)
            .ok_or_else(|| format!("no console `{name}` records"))
    }

    /// Finish the console `name`: its operation cancelled, its worker
    /// joined, its results kept.
    pub fn finish(&mut self, name: &str) -> Result<&Recording, String> {
        let at = self
            .recording
            .iter()
            .position(|(held, _, _)| *held == name)
            .ok_or_else(|| format!("no console `{name}` records"))?;
        let (name, file, recorder) = self.recording.remove(at);
        self.finished.push((name, file, recorder.finish()));
        self.finished
            .last()
            .map(|(_, _, recording)| recording)
            .ok_or_else(|| "no finished console".to_owned())
    }

    /// Finish every console still recording; every console's results, by
    /// name, and whether all are clean.
    pub fn finish_all(mut self) -> (serde_json::Value, Result<(), String>) {
        for (name, file, recorder) in std::mem::take(&mut self.recording) {
            self.finished.push((name, file, recorder.finish()));
        }
        let mut record = serde_json::Map::new();
        let mut unclean = Vec::new();
        for (name, file, recording) in &self.finished {
            record.insert((*name).to_owned(), recording.record(file));
            if let Err(error) = recording.clean() {
                unclean.push(format!("{name}: {error}"));
            }
        }
        let clean = if unclean.is_empty() {
            Ok(())
        } else {
            Err(format!("consoles: {}", unclean.join("; ")))
        };
        (serde_json::Value::Object(record), clean)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An operation fed by the test a step at a time; the service gone once
    /// the sender drops, as the LAB client reports it. Its cancels are
    /// reported on `cancels`.
    struct Fed {
        events: mpsc::Receiver<Event>,
        cancels: mpsc::Sender<()>,
    }

    struct FedCancel(mpsc::Sender<()>);

    impl Cancel for FedCancel {
        fn cancel(&self) -> crate::sniffer::Result<()> {
            let _ = self.0.send(());
            Ok(())
        }
    }

    impl Operation for Fed {
        type Cancel = FedCancel;

        fn canceller(&self) -> FedCancel {
            FedCancel(self.cancels.clone())
        }

        fn cancel(&self) -> crate::sniffer::Result<()> {
            let _ = self.cancels.send(());
            Ok(())
        }

        fn event(&mut self) -> Event {
            self.events
                .recv()
                .unwrap_or_else(|_| Event::Finished(Err(SERVICE_GONE.to_owned())))
        }
    }

    /// A file the test reads while the recorder writes it: it refuses its
    /// writes from write number `refuses_from` on, or panics at write
    /// number `panics_at`.
    #[derive(Clone, Default)]
    struct File {
        bytes: Arc<Mutex<Vec<u8>>>,
        writes: Arc<Mutex<usize>>,
        refuses_from: Option<usize>,
        panics_at: Option<usize>,
    }

    impl Write for File {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let write = {
                let mut writes = self.writes.lock().unwrap();
                *writes += 1;
                *writes
            };
            if self.panics_at == Some(write) {
                panic!("the test's file panics at write {write}");
            }
            if self.refuses_from.is_some_and(|from| write >= from) {
                return Err(std::io::Error::other("disk full"));
            }
            self.bytes.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl File {
        fn text(&self) -> String {
            String::from_utf8(self.bytes.lock().unwrap().clone()).unwrap()
        }
    }

    /// A recorder over a fed operation into `file`: the sender of its
    /// events and the receiver of its cancels. Dropping the sender (the
    /// service gone) ends its worker on every path.
    fn record(file: &File) -> (Recorder<FedCancel>, mpsc::Sender<Event>, mpsc::Receiver<()>) {
        let (events, fed) = mpsc::channel();
        let (cancel, cancels) = mpsc::channel();
        let recorder = Recorder::start(
            "beacon-stamp",
            Fed {
                events: fed,
                cancels: cancel,
            },
            file.clone(),
        );
        (recorder, events, cancels)
    }

    fn output(text: &str) -> Event {
        Event::Output(text.as_bytes().to_vec())
    }

    /// The ROM's lines of a boot's start, its reset line among them.
    const ROM: &str = "ESP-ROM:esp32s31-20251218\nBuild:Dec 18 2025\n\
                       rst:0x17 (CHIP_USB_UART_RESET),boot:0x58 (SPI_FAST_FLASH_BOOT)\n";

    #[test]
    fn every_byte_is_in_the_file_before_its_line_is_read() {
        let file = File::default();
        let (recorder, events, cancels) = record(&file);
        // An earlier boot's fragment, the ROM, a line split across chunks,
        // then the start of the next one.
        events
            .send(output(&format!("I (195) esp_image{ROM}STAMP v1 be")))
            .unwrap();
        events.send(output("gin run=1\nSTAMP roun")).unwrap();
        assert_eq!(recorder.wait_for("STAMP v1 begin", Duration::MAX), Ok(()));
        // The unfinished tail is in the file already, never read as a line.
        assert_eq!(
            file.text(),
            format!("I (195) esp_image{ROM}STAMP v1 begin run=1\nSTAMP roun")
        );
        events
            .send(Event::Finished(Ok(serde_json::json!("receipt"))))
            .unwrap();
        let recording = recorder.finish();
        assert_eq!(cancels.try_recv(), Ok(()));
        assert_eq!(recording.written, file.text().len() as u64);
        assert_eq!(recording.write, Outcome::Ok);
        assert_eq!((recording.lost, recording.read.clone()), (0, Outcome::Ok));
        assert_eq!(
            recording.end,
            End::Reported(Ok(serde_json::json!("receipt")))
        );
        assert!(recording.clean().is_ok());
    }

    #[test]
    fn a_wait_that_gives_up_keeps_the_tail_and_the_end_still_joins() {
        let file = File::default();
        let (recorder, events, cancels) = record(&file);
        events
            .send(output(&format!("{ROM}STAMP boot\nhalf a li")))
            .unwrap();
        // No marker: the wait gives up at once; the tail is not lost.
        let refused = recorder.wait_for("ACKRESP listening", Duration::ZERO);
        assert!(refused.unwrap_err().contains("no `ACKRESP listening` line"));
        // The cancel the finish sends is what the service ends on.
        let finisher = std::thread::spawn(move || recorder.finish());
        assert_eq!(cancels.recv(), Ok(()));
        events.send(output("ne\n")).unwrap();
        events
            .send(Event::Finished(Err("cancelled".into())))
            .unwrap();
        let recording = finisher.join().unwrap();
        assert_eq!(file.text(), format!("{ROM}STAMP boot\nhalf a line\n"));
        assert_eq!(recording.end, End::Reported(Err("cancelled".into())));
        assert_eq!(recording.read, Outcome::Ok);
    }

    #[test]
    fn a_result_before_the_marker_is_refused_and_recorded() {
        let file = File::default();
        let (recorder, events, _cancels) = record(&file);
        events
            .send(output(&format!(
                "{ROM}OER-RESULT: fail beacon-stamp clocks\n"
            )))
            .unwrap();
        let refused = recorder
            .wait_for("STAMP v1 begin", Duration::MAX)
            .unwrap_err();
        assert!(refused.contains("ended before"), "{refused}");
        drop(events);
        let recording = recorder.finish();
        assert!(
            file.text()
                .ends_with("OER-RESULT: fail beacon-stamp clocks\n")
        );
        // The service gone: the read failed, the end is unknown.
        assert_eq!(recording.read, Outcome::Failed(SERVICE_GONE.to_owned()));
        assert!(matches!(recording.end, End::Unknown(_)));
    }

    #[test]
    fn the_owners_disk_full_refuses_the_marker_wait_and_keeps_its_cause_apart() {
        let file = File {
            refuses_from: Some(1),
            ..File::default()
        };
        let (recorder, events, cancels) = record(&file);
        events
            .send(output(&format!("{ROM}STAMP v1 begin\n")))
            .unwrap();
        // The marker came, but the file does not hold it: the wait fails
        // with the write.
        let refused = recorder
            .wait_for("STAMP v1 begin", Duration::MAX)
            .unwrap_err();
        assert!(
            refused.contains("record stopped") && refused.contains("disk full"),
            "{refused}"
        );
        // The worker cancelled the operation it can no longer record; the
        // finish and the other results still go on.
        assert_eq!(cancels.recv(), Ok(()));
        events.send(Event::Lost(7)).unwrap();
        events
            .send(Event::Finished(Err("the operation was cancelled".into())))
            .unwrap();
        let recording = recorder.finish();
        assert_eq!(recording.written, 0);
        assert!(matches!(&recording.write, Outcome::Failed(error) if error.contains("disk full")));
        assert_eq!(recording.lost, 7);
        assert_eq!(recording.read, Outcome::Ok);
        assert_eq!(
            recording.end,
            End::Reported(Err("the operation was cancelled".into()))
        );
    }

    #[test]
    fn the_owners_old_result_prefix_is_not_the_new_boots_result() {
        let file = File::default();
        let (recorder, events, _cancels) = record(&file);
        // The earlier boot's result, then the ROM and a new boot that has
        // not ended.
        events
            .send(output(&format!(
                "OER-RESULT: pass beacon-stamp\n{ROM}STAMP v1 begin run=2\n"
            )))
            .unwrap();
        drop(events);
        let refused = recorder.result(Duration::MAX).unwrap_err();
        assert!(refused.contains("ended before"), "{refused}");
        // The prefix stays in the file, raw.
        assert!(file.text().starts_with("OER-RESULT: pass beacon-stamp\n"));
        drop(recorder);
    }

    #[test]
    fn a_second_boot_refuses_every_wait() {
        let file = File::default();
        let (recorder, events, _cancels) = record(&file);
        events
            .send(output(&format!(
                "{ROM}STAMP v1 begin\n{ROM}OER-RESULT: pass beacon-stamp\n"
            )))
            .unwrap();
        assert_eq!(recorder.wait_for("STAMP v1 begin", Duration::MAX), Ok(()));
        let refused = recorder.result(Duration::MAX).unwrap_err();
        assert!(refused.contains("booted again"), "{refused}");
        assert!(
            recorder
                .wait_for("anything", Duration::MAX)
                .unwrap_err()
                .contains("booted again")
        );
        drop(events);
        drop(recorder);
    }

    #[test]
    fn a_console_without_a_boot_start_answers_no_wait() {
        let file = File::default();
        let (recorder, events, _cancels) = record(&file);
        events
            .send(output("STAMP v1 begin\nOER-RESULT: pass beacon-stamp\n"))
            .unwrap();
        drop(events);
        assert!(recorder.wait_for("STAMP v1 begin", Duration::MAX).is_err());
        drop(recorder);
    }

    #[test]
    fn the_owners_worker_panic_keeps_what_was_observed_and_marks_the_rest_unknown() {
        let file = File {
            panics_at: Some(2),
            ..File::default()
        };
        let (recorder, events, _cancels) = record(&file);
        // Six bytes confirmed written, seven lost, then a write that panics.
        events.send(output("rst:x\n")).unwrap();
        events.send(Event::Lost(7)).unwrap();
        events.send(output("STAMP\n")).unwrap();
        let ended = recorder.wait_for("STAMP", Duration::MAX).unwrap_err();
        assert!(ended.contains("ended before"), "{ended}");
        let recording = recorder.finish();
        assert_eq!(recording.written, 6);
        assert_eq!(recording.lost, 7);
        assert!(
            matches!(&recording.write, Outcome::Unknown(why) if why.contains("during a write of 6 bytes")),
            "{:?}",
            recording.write
        );
        assert!(
            matches!(&recording.read, Outcome::Failed(error) if error.contains("recorder failed"))
        );
        assert!(matches!(recording.end, End::Unknown(_)));
        assert!(recording.clean().is_err());
        drop(events);
    }

    #[test]
    fn a_console_that_ends_before_its_marker_ends_the_wait() {
        let file = File::default();
        let (recorder, events, _cancels) = record(&file);
        events.send(output(&format!("{ROM}STAMP boot"))).unwrap();
        events
            .send(Event::Finished(Err(
                "the console failed: broken pipe".into()
            )))
            .unwrap();
        let refused = recorder
            .wait_for("STAMP v1 begin", Duration::MAX)
            .unwrap_err();
        assert!(refused.contains("the console ended before"), "{refused}");
        let recording = recorder.finish();
        assert_eq!(file.text(), format!("{ROM}STAMP boot"));
        assert_eq!(
            recording.end,
            End::Reported(Err("the console failed: broken pipe".into()))
        );
    }

    #[test]
    fn a_dropped_recorder_cancels_and_joins_its_worker() {
        let file = File::default();
        let (recorder, events, cancels) = record(&file);
        events.send(output("STAMP boot\n")).unwrap();
        let dropper = std::thread::spawn(move || drop(recorder));
        assert_eq!(cancels.recv(), Ok(()));
        drop(events);
        dropper.join().unwrap();
        assert_eq!(file.text(), "STAMP boot\n");
    }

    #[test]
    fn every_console_is_finished_and_kept_whatever_ended_the_run() {
        let (preflight, responder) = (File::default(), File::default());
        let (first, first_events, _first_cancels) = record(&preflight);
        let (second, second_events, second_cancels) = record(&responder);
        let mut consoles = Consoles::new();
        consoles.add("preflight", "p.log".into(), first);
        consoles.add("responder", "r.log".into(), second);
        first_events
            .send(output(&format!("{ROM}STAMP v1 begin\n")))
            .unwrap();
        first_events
            .send(Event::Finished(Ok(serde_json::json!("receipt"))))
            .unwrap();
        assert!(
            consoles
                .get("preflight")
                .unwrap()
                .wait_for("begin", Duration::MAX)
                .is_ok()
        );
        assert!(consoles.finish("preflight").unwrap().clean().is_ok());
        assert!(consoles.get("preflight").is_err());
        // The run ends early: the responder is still recording; its finish
        // cancels it, and its service then ends it with a loss reported.
        let ender = std::thread::spawn(move || consoles.finish_all());
        assert_eq!(second_cancels.recv(), Ok(()));
        second_events.send(Event::Lost(3)).unwrap();
        second_events
            .send(Event::Finished(Ok(serde_json::Value::Null)))
            .unwrap();
        let (record, clean) = ender.join().unwrap();
        assert_eq!(record["preflight"]["file"], "p.log");
        assert_eq!(record["preflight"]["end"]["ok"], true);
        assert_eq!(record["responder"]["lost"], 3);
        let unclean = clean.unwrap_err();
        assert!(unclean.contains("responder: lost: 3 bytes"), "{unclean}");
        assert!(!unclean.contains("preflight"), "{unclean}");
    }
}
