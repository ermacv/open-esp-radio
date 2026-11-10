//! The run's capture on the stand's sniffer, the mt76 router's monitor, as
//! part of the lease (`RouterOperation::MonitorCapture`, `tools/lab`).
//!
//! Within the lease the capture starts first, and its start returns only
//! once `tcpdump` listens (the service's `Ready`); then the probe is written,
//! and its first boot after the write runs and transmits at once. A short
//! preflight (`preflight.rs`) reads the capture of that boot's frames:
//! only when it accepts them is the probe reset and its console read from
//! the reset to its result line, the run the decision uses. The
//! capture stops after that or whatever ended the run early: a started
//! capture is stopped on every outcome. The run's result, the capture's
//! stop and the lease's release are each kept, and recorded apart; a
//! failure of one never hides another. The service's capture removes its
//! own monitor interface on every ending, and its park removes any
//! interface on the radio at the lease's end.
//!
//! The stream is the pcap bytes alone: output the client could not take
//! ends the capture there, and any byte reported lost makes the capture
//! fail, never a file with a gap.

use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, String>;

/// One capture's life on a sniffer, over a context `C` it shares with the
/// run (the LAB client).
pub trait Sniffer<C> {
    /// Start capturing `channel` into `file`.
    fn start(&mut self, context: &mut C, channel: u8, file: &Path) -> Result<()>;
    /// The capture's bytes so far, for the preflight.
    fn captured(&mut self) -> Result<Vec<u8>>;
    /// Stop the capture started last; the file it wrote.
    fn stop(&mut self) -> Result<PathBuf>;
}

/// What a captured run left: the body's result (`None`: it did not run)
/// and the capture's.
#[derive(Debug, Eq, PartialEq)]
pub struct Captured<T> {
    pub body: Option<Result<T>>,
    pub capture: Result<PathBuf>,
}

/// Run `body` under a capture of `channel` into `file`: a refused start
/// runs nothing; a started capture is stopped whatever `body` returned.
/// `body` reads the capture through the sniffer for its preflight.
pub fn with_capture<C, S: Sniffer<C>, T>(
    context: &mut C,
    sniffer: &mut S,
    channel: u8,
    file: &Path,
    body: impl FnOnce(&mut C, &mut S) -> Result<T>,
) -> Captured<T> {
    if let Err(error) = sniffer.start(context, channel, file) {
        return Captured {
            body: None,
            capture: Err(format!("the capture did not start: {error}")),
        };
    }
    let body = body(context, sniffer);
    Captured {
        body: Some(body),
        capture: sniffer.stop(),
    }
}

/// A started LAB operation as the capture reads it: its events, and a
/// handle that cancels it from another thread.
pub trait Operation: Send + 'static {
    type Cancel: Cancel + Send + 'static;
    fn canceller(&self) -> Self::Cancel;
    fn cancel(&self) -> Result<()>;
    fn event(&mut self) -> oer_tools_lab_client::client::Event;
}

/// Ask the service to stop an operation; its end still follows.
pub trait Cancel {
    fn cancel(&self) -> Result<()>;
}

impl Operation for oer_tools_lab_client::client::Running {
    type Cancel = oer_tools_lab_client::client::Input;

    fn canceller(&self) -> Self::Cancel {
        self.input()
    }

    fn cancel(&self) -> Result<()> {
        Self::cancel(self)
    }

    fn event(&mut self) -> oer_tools_lab_client::client::Event {
        Self::event(self)
    }
}

impl Cancel for oer_tools_lab_client::client::Input {
    fn cancel(&self) -> Result<()> {
        Self::cancel(self)
    }
}

/// The stand's router through the LAB service: the capture's stream goes
/// to the file by a thread of its own until the operation finishes. Its
/// start returns only once the capture listens (`Event::Ready`), so the
/// run's body (the flashing first) never starts before it.
pub struct RouterMonitor<O: Operation = oer_tools_lab_client::client::Running> {
    running: Option<Running<O::Cancel>>,
}

struct Running<C> {
    cancel: C,
    reader: std::thread::JoinHandle<Result<serde_json::Value>>,
    file: PathBuf,
}

impl<O: Operation> RouterMonitor<O> {
    pub const fn new() -> Self {
        Self { running: None }
    }

    fn captured_bytes(&self) -> Result<Vec<u8>> {
        let running = self.running.as_ref().ok_or("no capture runs")?;
        std::fs::read(&running.file).map_err(|error| format!("{}: {error}", running.file.display()))
    }

    fn end(&mut self) -> Result<PathBuf> {
        let running = self.running.take().ok_or("no capture was started")?;
        let cancelled = running.cancel.cancel();
        let report = running
            .reader
            .join()
            .map_err(|_| "the capture reader failed".to_owned())
            .and_then(|result| result);
        match (cancelled, report) {
            (Ok(()), Ok(report)) => {
                eprintln!("beacon-stamp: capture stopped: {report}");
                Ok(running.file)
            }
            (cancelled, report) => Err(format!(
                "cancel: {}; capture: {}",
                cancelled.map_or_else(|e| e, |()| "sent".into()),
                report.map_or_else(|e| e, |report| report.to_string())
            )),
        }
    }
}

/// Read `operation`'s stream into `output` on a thread of its own, and
/// return once the capture listens. An operation that ends before (a
/// capture that never listened, a stream the file refused, which cancels
/// it) is joined, its reader too, and refused with its end: the service's
/// capture removed what it created on that end. The wait is bounded by
/// the service's own: its readiness deadline ends the operation.
fn attach<O: Operation>(
    mut operation: O,
    mut output: impl std::io::Write + Send + 'static,
    file: &Path,
) -> Result<Running<O::Cancel>> {
    use oer_tools_lab_client::client::Event;
    let cancel = operation.canceller();
    let (ready, listening) = std::sync::mpsc::channel::<()>();
    let reader = std::thread::spawn(move || {
        let mut written = Ok(());
        let mut lost = 0u64;
        loop {
            match operation.event() {
                Event::Output(bytes) => {
                    if written.is_ok() {
                        written = output
                            .write_all(&bytes)
                            .map_err(|error| format!("the capture file: {error}"));
                        if written.is_err() {
                            let _ = operation.cancel();
                        }
                    }
                }
                Event::Ready => {
                    let _ = ready.send(());
                }
                Event::Lost(bytes) => lost = lost.saturating_add(bytes),
                Event::Finished(result) => {
                    let flushed = output
                        .flush()
                        .map_err(|error| format!("the capture file: {error}"));
                    return match (result, written, flushed, lost) {
                        (Ok(report), Ok(()), Ok(()), 0) => Ok(report),
                        (result, written, flushed, lost) => Err(format!(
                            "router: {}; file: {}; lost bytes: {lost}",
                            result.map_or_else(|e| e, |report| report.to_string()),
                            written
                                .and(flushed)
                                .map_or_else(|e| e, |()| "written".into()),
                        )),
                    };
                }
            }
        }
    });
    if listening.recv().is_ok() {
        return Ok(Running {
            cancel,
            reader,
            file: file.to_owned(),
        });
    }
    // The reader ended without a readiness: the operation finished, or the
    // reader failed with the operation still running, which the cancel
    // ends.
    let ended = reader
        .join()
        .map_err(|_| "the capture reader failed".to_owned())
        .and_then(|result| result);
    let cancelled = cancel.cancel();
    Err(format!(
        "the capture ended before it listened: {}; cancel: {}",
        ended.map_or_else(
            |e| e,
            |report| format!("finished without readiness: {report}")
        ),
        cancelled.map_or_else(|e| e, |()| "sent".into())
    ))
}

impl Sniffer<oer_tools_lab_client::client::Client> for RouterMonitor {
    fn start(
        &mut self,
        client: &mut oer_tools_lab_client::client::Client,
        channel: u8,
        file: &Path,
    ) -> Result<()> {
        use oer_tools_lab_client::protocol::{Operation, RouterOperation};
        let output =
            std::fs::File::create(file).map_err(|error| format!("{}: {error}", file.display()))?;
        let running = client.start(Operation::Router {
            request: RouterOperation::MonitorCapture { channel },
        })?;
        self.running = Some(attach(running, output, file)?);
        Ok(())
    }

    fn captured(&mut self) -> Result<Vec<u8>> {
        self.captured_bytes()
    }

    fn stop(&mut self) -> Result<PathBuf> {
        self.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sniffer that refuses its start or its stop as told and records
    /// every call; its context counts the body's runs.
    struct Fake {
        refuse_start: bool,
        refuse_stop: bool,
        calls: Vec<&'static str>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                refuse_start: false,
                refuse_stop: false,
                calls: Vec::new(),
            }
        }
    }

    impl Sniffer<u32> for Fake {
        fn start(&mut self, _: &mut u32, channel: u8, _: &Path) -> Result<()> {
            assert_eq!(channel, 13);
            self.calls.push("start");
            if self.refuse_start {
                Err("no monitor".into())
            } else {
                Ok(())
            }
        }

        fn captured(&mut self) -> Result<Vec<u8>> {
            self.calls.push("captured");
            Ok(Vec::new())
        }

        fn stop(&mut self) -> Result<PathBuf> {
            self.calls.push("stop");
            if self.refuse_stop {
                Err("tcpdump lost".into())
            } else {
                Ok(PathBuf::from("run.pcap"))
            }
        }
    }

    fn run<T>(sniffer: &mut Fake, preflight: Result<()>, body: Result<T>) -> (Captured<T>, u32) {
        let mut runs = 0u32;
        let captured = with_capture(
            &mut runs,
            sniffer,
            13,
            Path::new("run.pcap"),
            |runs, sniffer| {
                let _ = sniffer.captured();
                preflight?;
                *runs += 1;
                body
            },
        );
        (captured, runs)
    }

    #[test]
    fn a_refused_start_runs_nothing() {
        let mut sniffer = Fake::new();
        sniffer.refuse_start = true;
        let (captured, runs) = run(&mut sniffer, Ok(()), Ok(()));
        assert_eq!(runs, 0);
        assert_eq!(captured.body, None);
        assert!(captured.capture.unwrap_err().contains("no monitor"));
        assert_eq!(sniffer.calls, ["start"]);
    }

    #[test]
    fn a_refused_preflight_runs_nothing_and_still_stops_the_capture() {
        let mut sniffer = Fake::new();
        let (captured, runs) = run::<()>(&mut sniffer, Err("no radiotap TSFT".into()), Ok(()));
        assert_eq!(runs, 0);
        assert_eq!(captured.body, Some(Err("no radiotap TSFT".into())));
        assert!(captured.capture.is_ok());
        assert_eq!(sniffer.calls, ["start", "captured", "stop"]);
    }

    #[test]
    fn a_started_capture_is_stopped_after_a_completed_run() {
        let mut sniffer = Fake::new();
        let (captured, runs) = run(&mut sniffer, Ok(()), Ok(true));
        assert_eq!(runs, 1);
        assert_eq!(captured.body, Some(Ok(true)));
        assert_eq!(captured.capture, Ok(PathBuf::from("run.pcap")));
        assert_eq!(sniffer.calls, ["start", "captured", "stop"]);
    }

    #[test]
    fn a_failed_run_and_a_failed_stop_keep_both_causes() {
        let mut sniffer = Fake::new();
        sniffer.refuse_stop = true;
        let (captured, _) = run::<bool>(&mut sniffer, Ok(()), Err("flash refused".into()));
        assert_eq!(captured.body, Some(Err("flash refused".into())));
        assert_eq!(captured.capture, Err("tcpdump lost".into()));
        assert_eq!(sniffer.calls, ["start", "captured", "stop"]);
    }

    /// A LAB operation fed by the test a step at a time: each event it is
    /// sent, and the service gone once its sender drops. It reports what
    /// happens to the test on one channel.
    mod feed {
        use oer_tools_lab_client::client::Event;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        };

        #[derive(Debug, PartialEq)]
        pub enum Signal {
            Cancelled,
            BodyRan { saw_ready: bool },
            WorkerDone,
        }

        pub struct FakeOperation {
            pub events: mpsc::Receiver<Event>,
            pub signals: mpsc::Sender<Signal>,
            pub ready: Arc<AtomicBool>,
        }

        pub struct FakeCancel(pub mpsc::Sender<Signal>);

        impl super::super::Cancel for FakeCancel {
            fn cancel(&self) -> super::Result<()> {
                let _ = self.0.send(Signal::Cancelled);
                Ok(())
            }
        }

        impl super::super::Operation for FakeOperation {
            type Cancel = FakeCancel;

            fn canceller(&self) -> FakeCancel {
                FakeCancel(self.signals.clone())
            }

            fn cancel(&self) -> super::Result<()> {
                let _ = self.signals.send(Signal::Cancelled);
                Ok(())
            }

            fn event(&mut self) -> Event {
                match self.events.recv() {
                    Ok(Event::Ready) => {
                        self.ready.store(true, Ordering::SeqCst);
                        Event::Ready
                    }
                    Ok(event) => event,
                    Err(_) => Event::Finished(Err("the service is gone".into())),
                }
            }
        }

        /// The context of a run: the operation its start takes.
        pub struct Context {
            pub operation: Option<FakeOperation>,
        }
    }

    use feed::{Context, FakeOperation, Signal};
    use oer_tools_lab_client::client::Event;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    impl Sniffer<Context> for RouterMonitor<FakeOperation> {
        fn start(&mut self, context: &mut Context, _: u8, file: &Path) -> Result<()> {
            let operation = context.operation.take().ok_or("started twice")?;
            let output = std::fs::File::create(file).map_err(|error| error.to_string())?;
            self.running = Some(attach(operation, output, file)?);
            Ok(())
        }

        fn captured(&mut self) -> Result<Vec<u8>> {
            self.captured_bytes()
        }

        fn stop(&mut self) -> Result<PathBuf> {
            self.end()
        }
    }

    /// A run over a fed operation in a worker: the test sends the events
    /// and reads the signals; the body reports whether the capture had
    /// listened when it ran.
    struct Run {
        events: Option<mpsc::Sender<Event>>,
        signals: mpsc::Receiver<Signal>,
        worker: Option<std::thread::JoinHandle<Captured<()>>>,
        _directory: tempfile::TempDir,
        file: PathBuf,
    }

    impl Run {
        fn start() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let file = directory.path().join("run.pcap");
            let (events, fed) = mpsc::channel();
            let (signal, signals) = mpsc::channel();
            let ready = Arc::new(AtomicBool::new(false));
            let operation = FakeOperation {
                events: fed,
                signals: signal.clone(),
                ready: ready.clone(),
            };
            let path = file.clone();
            let worker = std::thread::spawn(move || {
                let mut context = Context {
                    operation: Some(operation),
                };
                let mut monitor = RouterMonitor::<FakeOperation>::new();
                let body_signal = signal.clone();
                let captured = with_capture(&mut context, &mut monitor, 13, &path, |_, _| {
                    let _ = body_signal.send(Signal::BodyRan {
                        saw_ready: ready.load(Ordering::SeqCst),
                    });
                    Ok(())
                });
                let _ = signal.send(Signal::WorkerDone);
                captured
            });
            Self {
                events: Some(events),
                signals,
                worker: Some(worker),
                _directory: directory,
                file,
            }
        }

        fn send(&self, event: Event) {
            let _ = self.events.as_ref().unwrap().send(event);
        }

        fn next(&self) -> Signal {
            self.signals.recv().unwrap()
        }

        /// The worker's result, once it said it is done.
        fn join(mut self) -> (Captured<()>, Vec<u8>) {
            drop(self.events.take());
            let captured = self.worker.take().unwrap().join().unwrap();
            let bytes = std::fs::read(&self.file).unwrap_or_default();
            (captured, bytes)
        }
    }

    impl Drop for Run {
        fn drop(&mut self) {
            // The service gone: every wait of the worker ends.
            drop(self.events.take());
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    #[test]
    fn the_body_runs_only_once_the_capture_listens() {
        let run = Run::start();
        run.send(Event::Output(b"pcap-header".to_vec()));
        run.send(Event::Ready);
        // The body ran after the readiness; the stop cancels; the service
        // then ends the operation.
        assert_eq!(run.next(), Signal::BodyRan { saw_ready: true });
        assert_eq!(run.next(), Signal::Cancelled);
        run.send(Event::Output(b"tail".to_vec()));
        run.send(Event::Finished(Ok(serde_json::json!({"bytes": 15}))));
        assert_eq!(run.next(), Signal::WorkerDone);
        let (captured, bytes) = run.join();
        assert_eq!(captured.body, Some(Ok(())));
        assert!(captured.capture.is_ok(), "{:?}", captured.capture);
        assert_eq!(bytes, b"pcap-headertail");
    }

    #[test]
    fn a_capture_that_ends_before_it_listens_runs_no_body_and_is_joined() {
        let run = Run::start();
        run.send(Event::Output(b"pcap-header".to_vec()));
        run.send(Event::Finished(Err(
            "the capture did not become ready: tcpdump did not report that it listens; \
             interface removal: confirmed"
                .into(),
        )));
        // Joined, then cancelled for an operation that may still run.
        assert_eq!(run.next(), Signal::Cancelled);
        assert_eq!(run.next(), Signal::WorkerDone);
        let (captured, _) = run.join();
        assert_eq!(captured.body, None);
        let error = captured.capture.unwrap_err();
        assert!(
            error.contains("ended before it listened")
                && error.contains("interface removal: confirmed"),
            "{error}"
        );
    }

    #[test]
    fn a_stream_the_file_refuses_before_readiness_cancels_and_refuses_the_start() {
        struct Refusing;
        impl std::io::Write for Refusing {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (events, fed) = mpsc::channel();
        let (signal, signals) = mpsc::channel();
        let operation = FakeOperation {
            events: fed,
            signals: signal,
            ready: Arc::new(AtomicBool::new(false)),
        };
        let attacher =
            std::thread::spawn(move || attach(operation, Refusing, Path::new("run.pcap")).err());
        events.send(Event::Output(b"pcap-header".to_vec())).unwrap();
        // The reader cancels the operation, which then ends.
        assert_eq!(signals.recv().unwrap(), Signal::Cancelled);
        events
            .send(Event::Finished(Err(
                "cancelled; interface removal: confirmed".into(),
            )))
            .unwrap();
        let error = attacher.join().unwrap().unwrap();
        assert!(
            error.contains("ended before it listened") && error.contains("disk full"),
            "{error}"
        );
    }
}
