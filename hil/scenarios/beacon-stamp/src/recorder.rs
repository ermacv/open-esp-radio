//! A board console recorded continuously: a worker of its own writes every
//! byte of the LAB operation's stream to the console's file as it arrives,
//! flushed, before it hands the stream's complete lines to whoever waits for
//! a marker. A wait that gives up (a timeout, a result before its marker,
//! the console's end) loses no byte: the file already holds everything the
//! stream delivered. Every ending cancels the operation and joins the
//! worker, and keeps four results apart: the file's write, the bytes the
//! service dropped for a slow reader, the stream's own read, and the
//! operation's end.

use crate::{
    result::{ProbeResult, parse},
    sniffer::{Cancel, Operation},
};
use oer_tools_lab_client::client::Event;
use std::{
    io::Write,
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// The LAB client's end of an operation whose service went away
/// (`oer_tools_lab_client::client`): the stream's read failed, the
/// operation's own end is unknown.
const SERVICE_GONE: &str = "the LAB service closed the connection";

/// What one recorded console left, each result its own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recording {
    /// Bytes the file took.
    pub written: u64,
    /// The file's first failed write; the bytes after it are not in it.
    pub write: Result<(), String>,
    /// Bytes the service dropped for a slow reader (`Event::Lost`).
    pub lost: u64,
    /// The stream's read: the service gone before the operation's end, or
    /// the worker failed.
    pub read: Result<(), String>,
    /// The operation's end, as the service reported it; `None` when the
    /// read failed before it.
    pub end: Option<Result<serde_json::Value, String>>,
}

impl Recording {
    /// The recording as the run's outcome records it.
    pub fn record(&self, file: &str) -> serde_json::Value {
        let result = |result: &Result<(), String>| match result {
            Ok(()) => serde_json::json!({ "ok": true }),
            Err(error) => serde_json::json!({ "ok": false, "detail": error }),
        };
        serde_json::json!({
            "file": file,
            "written": self.written,
            "write": result(&self.write),
            "lost": self.lost,
            "read": result(&self.read),
            "end": match &self.end {
                None => serde_json::json!({ "ok": false, "detail": "not reported" }),
                Some(Ok(value)) => serde_json::json!({ "ok": true, "detail": value }),
                Some(Err(error)) => serde_json::json!({ "ok": false, "detail": error }),
            },
        })
    }
}

pub struct Recorder<C: Cancel + Send + 'static> {
    /// The image whose result line ends a result wait.
    image: &'static str,
    lines: mpsc::Receiver<String>,
    cancel: C,
    worker: Option<JoinHandle<Recording>>,
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
        let worker = std::thread::spawn(move || {
            let mut recording = Recording {
                written: 0,
                write: Ok(()),
                lost: 0,
                read: Ok(()),
                end: None,
            };
            let mut line = Vec::new();
            loop {
                match operation.event() {
                    Event::Output(bytes) => {
                        if recording.write.is_ok() {
                            recording.write = file
                                .write_all(&bytes)
                                .and_then(|()| file.flush())
                                .map_err(|error| format!("the console file: {error}"));
                            if recording.write.is_ok() {
                                recording.written =
                                    recording.written.saturating_add(bytes.len() as u64);
                            } else {
                                // The record has a gap from here: stop it.
                                let _ = operation.cancel();
                            }
                        }
                        // In the file first, then to the waiter.
                        for byte in bytes {
                            if byte == b'\n' {
                                let text = String::from_utf8_lossy(&line).trim_end().to_owned();
                                let _ = send.send(text);
                                line.clear();
                            } else {
                                line.push(byte);
                            }
                        }
                    }
                    Event::Lost(bytes) => recording.lost = recording.lost.saturating_add(bytes),
                    Event::Ready => {}
                    Event::Finished(result) => {
                        if !line.is_empty() {
                            let _ = send.send(String::from_utf8_lossy(&line).trim_end().to_owned());
                        }
                        match result {
                            Err(error) if error == SERVICE_GONE => recording.read = Err(error),
                            result => recording.end = Some(result),
                        }
                        return recording;
                    }
                }
            }
        });
        Self {
            image,
            lines,
            cancel,
            worker: Some(worker),
        }
    }

    /// The next line within `deadline` (none: no deadline), or why none
    /// came.
    fn line(&self, deadline: Option<Instant>, waiting: &str) -> Result<String, String> {
        let ended = || format!("the console ended before {waiting}");
        match deadline {
            None => self.lines.recv().map_err(|_| ended()),
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                self.lines.recv_timeout(left).map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => {
                        format!("no {waiting} within the console timeout")
                    }
                    mpsc::RecvTimeoutError::Disconnected => ended(),
                })
            }
        }
    }

    /// Wait until a line holds `marker`, within `timeout`; a result line
    /// before it is an error naming that result.
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

    /// Wait for the image's result line within `timeout`.
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
        match (joined, cancelled) {
            (Ok(recording), Ok(())) => recording,
            (Ok(mut recording), Err(error)) => {
                let read = recording.read.clone().err();
                recording.read = Err(match read {
                    Some(read) => format!("{read}; the cancel failed: {error}"),
                    None => format!("the cancel failed: {error}"),
                });
                recording
            }
            (Err(error), cancelled) => Recording {
                written: 0,
                write: Ok(()),
                lost: 0,
                read: Err(match cancelled {
                    Ok(()) => error,
                    Err(cancel) => format!("{error}; the cancel failed: {cancel}"),
                }),
                end: None,
            },
        }
    }
}

impl<C: Cancel + Send + 'static> Drop for Recorder<C> {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.end();
        }
    }
}

impl Recording {
    /// Whether nothing of the console is missing and its operation ended
    /// as asked; otherwise every cause, apart.
    pub fn clean(&self) -> Result<(), String> {
        let mut causes = Vec::new();
        if let Err(error) = &self.write {
            causes.push(format!("write: {error}"));
        }
        if self.lost > 0 {
            causes.push(format!("lost: {} bytes", self.lost));
        }
        if let Err(error) = &self.read {
            causes.push(format!("read: {error}"));
        }
        match &self.end {
            Some(Ok(_)) => {}
            Some(Err(error)) => causes.push(format!("end: {error}")),
            None => causes.push("end: not reported".to_owned()),
        }
        if causes.is_empty() {
            Ok(())
        } else {
            Err(causes.join("; "))
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
    use std::sync::{Arc, Mutex};

    /// An operation fed by the test a step at a time; the service gone once
    /// the sender drops, as the LAB client reports it. Its cancel is
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

    /// A file the test reads while the recorder writes it, or one that
    /// refuses every write.
    #[derive(Clone, Default)]
    struct File {
        bytes: Arc<Mutex<Vec<u8>>>,
        refuses: bool,
    }

    impl Write for File {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.refuses {
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

    #[test]
    fn every_byte_is_in_the_file_before_its_line_is_read() {
        let file = File::default();
        let (recorder, events, cancels) = record(&file);
        // A line split across chunks, then the start of the next one.
        events
            .send(output("ESP-ROM:esp32s31\nSTAMP v1 be"))
            .unwrap();
        events.send(output("gin run=1\nSTAMP roun")).unwrap();
        assert_eq!(recorder.wait_for("STAMP v1 begin", Duration::MAX), Ok(()));
        // The unfinished tail is in the file already, never read as a line.
        assert_eq!(
            file.text(),
            "ESP-ROM:esp32s31\nSTAMP v1 begin run=1\nSTAMP roun"
        );
        events
            .send(Event::Finished(Ok(serde_json::json!("receipt"))))
            .unwrap();
        let recording = recorder.finish();
        assert_eq!(cancels.try_recv(), Ok(()));
        assert_eq!(recording.written, file.text().len() as u64);
        assert_eq!(recording.write, Ok(()));
        assert_eq!((recording.lost, recording.read.clone()), (0, Ok(())));
        assert_eq!(recording.end, Some(Ok(serde_json::json!("receipt"))));
    }

    #[test]
    fn a_wait_that_gives_up_keeps_the_tail_and_the_end_still_joins() {
        let file = File::default();
        let (recorder, events, cancels) = record(&file);
        events.send(output("STAMP boot\nhalf a li")).unwrap();
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
        assert_eq!(file.text(), "STAMP boot\nhalf a line\n");
        assert_eq!(recording.end, Some(Err("cancelled".into())));
        assert_eq!(recording.read, Ok(()));
    }

    #[test]
    fn a_result_before_the_marker_is_refused_and_recorded() {
        let file = File::default();
        let (recorder, events, _cancels) = record(&file);
        events
            .send(output("OER-RESULT: fail beacon-stamp clocks\n"))
            .unwrap();
        let refused = recorder
            .wait_for("STAMP v1 begin", Duration::MAX)
            .unwrap_err();
        assert!(refused.contains("ended before"), "{refused}");
        drop(events);
        let recording = recorder.finish();
        assert_eq!(file.text(), "OER-RESULT: fail beacon-stamp clocks\n");
        // The service gone: the read failed, the end is unknown.
        assert_eq!(recording.read, Err(SERVICE_GONE.to_owned()));
        assert_eq!(recording.end, None);
    }

    #[test]
    fn a_file_that_refuses_a_write_cancels_and_keeps_its_cause_apart() {
        let file = File {
            refuses: true,
            ..File::default()
        };
        let (recorder, events, cancels) = record(&file);
        events.send(output("STAMP v1 begin\n")).unwrap();
        // The worker cancels the operation it can no longer record.
        assert_eq!(cancels.recv(), Ok(()));
        events.send(Event::Lost(7)).unwrap();
        events
            .send(Event::Finished(Err("the operation was cancelled".into())))
            .unwrap();
        let recording = recorder.finish();
        assert_eq!(recording.written, 0);
        assert!(recording.write.unwrap_err().contains("disk full"));
        assert_eq!(recording.lost, 7);
        assert_eq!(recording.read, Ok(()));
        assert_eq!(
            recording.end,
            Some(Err("the operation was cancelled".into()))
        );
    }

    #[test]
    fn a_console_that_ends_before_its_marker_ends_the_wait() {
        let file = File::default();
        let (recorder, events, _cancels) = record(&file);
        events.send(output("STAMP boot")).unwrap();
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
        assert_eq!(file.text(), "STAMP boot");
        assert_eq!(
            recording.end,
            Some(Err("the console failed: broken pipe".into()))
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
        first_events.send(output("STAMP v1 begin\n")).unwrap();
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
