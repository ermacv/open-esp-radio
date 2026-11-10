//! The capture over a real pipe, the real controlled reader thread and the
//! real bounded channel, on events: a writer thread plays `tcpdump`, every
//! wait of the code under test goes through the manual clock, and each test
//! runs that code in a worker the test drives a quiet step at a time, at
//! most `STEPS` steps. A guard owns every handle that can unblock a worker:
//! on any failure it releases the clock (every wait times out), lets the
//! writer end and joins the worker and the writer.

use super::super::*;
use super::clock::ManualClock;
use std::{
    io::{PipeReader, Read, Write as _},
    os::fd::{AsFd, BorrowedFd},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};

const TAIL: &[u8] = b"the-pcap-tail";
const CHUNK: usize = 4096;
/// The controlled deadline of every test, in quiet steps.
const STEPS: usize = 64;

/// The pipe's read end as the reader sees it; it reports what it read and
/// whether it was dropped (the reader's thread ended).
struct Source {
    pipe: PipeReader,
    clock: ManualClock,
    dropped: Arc<AtomicBool>,
    fail_after: Option<u32>,
}

impl Read for Source {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if let Some(left) = self.fail_after.as_mut() {
            if *left == 0 {
                return Err(std::io::Error::other("ssh died"));
            }
            *left -= 1;
        }
        let count = self.pipe.read(buffer)?;
        self.clock.read(count as u64);
        Ok(count)
    }
}

impl AsFd for Source {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.pipe.as_fd()
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
        self.clock.poke();
    }
}

/// How the writer behaves after its chunks.
#[derive(Clone, Copy)]
struct Script {
    /// 4096-octet writes, each after the previous one was read.
    chunks: usize,
    /// On the stop signal: write the tail and close; otherwise hold the
    /// pipe open, silent, until a second signal.
    closes: bool,
    fail_after: Option<u32>,
}

struct Writer {
    signal: mpsc::Sender<()>,
    thread: JoinHandle<u64>,
}

/// The pipe, its writer thread and the source over its read end.
fn pipe(clock: &ManualClock, script: Script) -> (Source, Writer, Arc<AtomicBool>) {
    let (reader, mut writer) = std::io::pipe().unwrap();
    let (signal, signals) = mpsc::channel::<()>();
    clock.plan_writes(script.chunks as u64);
    let at = clock.clone();
    let thread = std::thread::spawn(move || {
        let chunk = [0x5au8; CHUNK];
        let mut written = 0u64;
        for _ in 0..script.chunks {
            at.wait_then_write(CHUNK as u64);
            let sent = writer.write_all(&chunk).is_ok();
            // The write is counted done only once its bytes are in the
            // pipe (or it failed): no quiet moment before they are readable.
            at.write_done();
            if !sent {
                return written;
            }
            written += CHUNK as u64;
        }
        // The stop signal, or a guard that dropped the sender.
        let _ = signals.recv();
        if script.closes {
            at.wrote(TAIL.len() as u64);
            if writer.write_all(TAIL).is_ok() {
                written += TAIL.len() as u64;
            }
            // The tail and the end are in the pipe before the signal
            // counts as acted on: no quiet moment before they are readable.
            drop(writer);
            at.signal_taken();
        } else {
            at.signal_taken();
            let _ = signals.recv();
            drop(writer);
        }
        written
    });
    let dropped = Arc::new(AtomicBool::new(false));
    clock.expect_reader();
    (
        Source {
            pipe: reader,
            clock: clock.clone(),
            dropped: dropped.clone(),
            fail_after: script.fail_after,
        },
        Writer { signal, thread },
        dropped,
    )
}

/// The process over the pipe: its stop is the writer's signal.
struct PipeProcess {
    stream: Stream,
    signal: mpsc::Sender<()>,
    clock: ManualClock,
    /// Chunks the reader must have delivered before the capture runs.
    ready_after: u64,
    exit_ok: bool,
    /// The scripted stderr's reader, and the signal that lets its script
    /// fail: sent at the stop request, dropped at the exit at the latest.
    stderr: Option<StderrReader>,
    stderr_signal: Option<mpsc::Sender<()>>,
    diagnostics: Diagnostics,
}

/// How a scripted stderr ends once the capture's stop was requested,
/// after its readiness.
#[derive(Clone, Copy, Debug)]
enum StderrEnd {
    ReadError,
    Panic,
}

/// A stderr that names its process, then waits for the stop request and
/// ends as scripted.
struct StderrScript {
    first: bool,
    signal: mpsc::Receiver<()>,
    end: StderrEnd,
}

const PID_LINE: &str = "oer-capture-pid 7\n";

impl Read for StderrScript {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if std::mem::take(&mut self.first) {
            buffer[..PID_LINE.len()].copy_from_slice(PID_LINE.as_bytes());
            return Ok(PID_LINE.len());
        }
        let _ = self.signal.recv();
        match self.end {
            StderrEnd::ReadError => Err(std::io::Error::other("ssh died")),
            StderrEnd::Panic => panic!("the scripted stderr reader panics"),
        }
    }
}

impl Process for PipeProcess {
    fn ready<C: Clock>(&mut self, _: &C, _: C::Instant) -> Result<()> {
        let until = self.clock.after(self.clock.now(), READY);
        if self.clock.until_delivered(self.ready_after, until) {
            Ok(())
        } else {
            Err("the channel never filled".into())
        }
    }

    fn next<C: Clock>(&mut self, clock: &C, until: C::Instant) -> Next {
        self.stream.next(clock, until)
    }

    fn stop(&mut self) -> Result<()> {
        self.clock.signal_sent();
        let _ = self.signal.send(());
        if let Some(signal) = &self.stderr_signal {
            let _ = signal.send(());
        }
        Ok(())
    }

    fn wait_exit<C: Clock>(&mut self, clock: &C, until: C::Instant) -> Result<String> {
        let read = self.stream.close(clock, until);
        // The process is gone: its stderr ends, and its reader is joined.
        drop(self.stderr_signal.take());
        if let Some(stderr) = self.stderr.take() {
            self.diagnostics = stderr.join();
        }
        let read = read?;
        if self.exit_ok {
            Ok(format!("exited (0), {read} bytes read"))
        } else {
            Err("the capture exited with 1".into())
        }
    }

    fn diagnostics(&mut self) -> Diagnostics {
        self.diagnostics.clone()
    }
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn push(&self, entry: String) {
        self.0.lock().unwrap().push(entry);
    }

    fn removed(&self) -> bool {
        self.0
            .lock()
            .unwrap()
            .ends_with(&["remove oercap0".to_owned(), "listing".to_owned()])
    }
}

struct PipeHost {
    process: Option<PipeProcess>,
    created: bool,
    log: Log,
}

impl Host for PipeHost {
    type Process = PipeProcess;

    fn listing(&mut self) -> Result<Listing> {
        self.log.push("listing".into());
        Ok(Listing {
            phy: "phy0".into(),
            on_radio: Vec::new(),
            all: if self.created {
                vec![INTERFACE.to_owned()]
            } else {
                Vec::new()
            },
        })
    }

    fn create(&mut self, _: &str, _: &str) -> Result<()> {
        self.created = true;
        Ok(())
    }

    fn isolate(&mut self, _: &str) -> Result<()> {
        Ok(())
    }

    fn up(&mut self, _: &str, _: u8) -> Result<()> {
        Ok(())
    }

    fn start(&mut self, _: &str) -> Result<PipeProcess> {
        self.process
            .take()
            .ok_or_else(|| "started twice".to_owned())
    }

    fn remove(&mut self, name: &str) -> Result<()> {
        self.log.push(format!("remove {name}"));
        self.created = false;
        Ok(())
    }
}

struct Sink {
    bytes: Vec<u8>,
    writes: usize,
    readied: usize,
    cancel_after: Option<usize>,
    fail_at: Option<usize>,
}

impl super::super::Sink for Sink {
    fn ready(&mut self) {
        self.readied += 1;
    }

    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writes += 1;
        if self.fail_at == Some(self.writes) {
            return Err("the client is gone".into());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn cancelled(&self) -> bool {
        self.cancel_after.is_some_and(|after| self.writes >= after)
    }
}

/// Every handle that can unblock a worker, used on every exit.
struct Guard<T> {
    clock: ManualClock,
    signal: Option<mpsc::Sender<()>>,
    writer: Option<JoinHandle<u64>>,
    worker: Option<JoinHandle<T>>,
    done: Arc<AtomicBool>,
}

impl<T> Guard<T> {
    /// Drive the worker to its end within `STEPS`, then join both threads:
    /// the worker's result and what the writer wrote.
    fn finish(mut self) -> (T, u64) {
        let finished = self.clock.drive(&self.done, STEPS);
        assert!(
            finished,
            "the worker did not end within {STEPS} quiet steps: {:?}",
            self.clock
        );
        let result = self.worker.take().unwrap().join().unwrap();
        // The writer may wait on bytes nobody reads any more.
        self.clock.release();
        drop(self.signal.take());
        let written = self.writer.take().unwrap().join().unwrap();
        (result, written)
    }
}

impl<T> Drop for Guard<T> {
    fn drop(&mut self) {
        self.clock.release();
        drop(self.signal.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

fn run<T: Send + 'static>(
    clock: &ManualClock,
    writer: Writer,
    work: impl FnOnce() -> T + Send + 'static,
) -> Guard<T> {
    let done = Arc::new(AtomicBool::new(false));
    let (flag, at) = (done.clone(), clock.clone());
    let worker = std::thread::spawn(move || {
        let result = work();
        flag.store(true, Ordering::SeqCst);
        at.poke();
        result
    });
    Guard {
        clock: clock.clone(),
        signal: Some(writer.signal),
        writer: Some(writer.thread),
        worker: Some(worker),
        done,
    }
}

struct Captured {
    result: Result<Report>,
    sink: Sink,
    log: Log,
}

/// A capture over a pipe of `script`, with `sink`, in a driven worker.
fn capture_over(
    script: Script,
    ready_after: u64,
    exit_ok: bool,
    sink: Sink,
) -> (Captured, u64, Arc<AtomicBool>) {
    capture_with_stderr(script, ready_after, exit_ok, sink, None)
}

/// [`capture_over`], with a scripted stderr when `stderr` names its end.
fn capture_with_stderr(
    script: Script,
    ready_after: u64,
    exit_ok: bool,
    sink: Sink,
    stderr: Option<StderrEnd>,
) -> (Captured, u64, Arc<AtomicBool>) {
    let clock = ManualClock::new();
    let (source, writer, dropped) = pipe(&clock, script);
    let stream = Stream::spawn(source, clock.clone()).unwrap();
    let (stderr, stderr_signal) = match stderr {
        None => (None, None),
        Some(end) => {
            let (signal, signals) = mpsc::channel();
            let script = StderrScript {
                first: true,
                signal: signals,
                end,
            };
            (Some(StderrReader::spawn(script, |_| {})), Some(signal))
        }
    };
    let process = PipeProcess {
        stream,
        signal: writer.signal.clone(),
        clock: clock.clone(),
        ready_after,
        exit_ok,
        stderr,
        stderr_signal,
        diagnostics: Diagnostics::default(),
    };
    let log = Log::default();
    let mut host = PipeHost {
        process: Some(process),
        created: false,
        log: log.clone(),
    };
    let at = clock.clone();
    let guard = run(&clock, writer, move || {
        let mut sink = sink;
        let result = capture(&mut host, 13, &mut sink, &at);
        drop(host);
        Captured { result, sink, log }
    });
    let (captured, written) = guard.finish();
    (captured, written, dropped)
}

fn sink() -> Sink {
    Sink {
        bytes: Vec::new(),
        writes: 0,
        readied: 0,
        cancel_after: None,
        fail_at: None,
    }
}

const FULL: Script = Script {
    chunks: 2 * STREAM_DEPTH,
    closes: true,
    fail_after: None,
};

#[test]
fn closing_a_stream_whose_source_never_answers_stops_and_joins_its_reader() {
    // The reader waits in `poll` on a pipe its writer holds open, silent.
    let clock = ManualClock::new();
    let (source, writer, dropped) = pipe(
        &clock,
        Script {
            chunks: 0,
            closes: false,
            fail_after: None,
        },
    );
    let mut stream = Stream::spawn(source, clock.clone()).unwrap();
    let at = clock.clone();
    let observed = dropped.clone();
    let guard = run(&clock, writer, move || {
        let until = at.after(at.now(), EXIT);
        let closed = stream.close(&at, until);
        // Joined: the reader's thread ended and freed its source.
        (closed, observed.load(Ordering::SeqCst))
    });
    let ((closed, joined), _) = guard.finish();
    assert!(closed.unwrap_err().contains("stopped"));
    assert!(
        joined,
        "close returned while the reader still held its source"
    );
}

#[test]
fn closing_a_stream_whose_reader_waits_on_a_full_channel_stops_and_joins_it() {
    let clock = ManualClock::new();
    let (source, writer, dropped) = pipe(&clock, FULL);
    let mut stream = Stream::spawn(source, clock.clone()).unwrap();
    let at = clock.clone();
    let observed = dropped.clone();
    let guard = run(&clock, writer, move || {
        let full = at.until_delivered(STREAM_DEPTH as u64, at.after(at.now(), READY));
        let closed = stream.close(&at, at.after(at.now(), EXIT));
        (full, closed, observed.load(Ordering::SeqCst))
    });
    let ((full, closed, joined), _) = guard.finish();
    assert!(full, "the channel never filled");
    assert!(closed.is_err());
    assert!(joined);
}

#[test]
fn a_cancel_with_the_channel_full_keeps_every_byte_and_the_tail() {
    let (captured, written, dropped) = capture_over(
        FULL,
        STREAM_DEPTH as u64,
        true,
        Sink {
            cancel_after: Some(0),
            ..sink()
        },
    );
    let report = captured.result.unwrap();
    assert!(captured.sink.bytes.ends_with(TAIL));
    assert_eq!(captured.sink.bytes.len() as u64, written);
    assert_eq!(report.bytes, written);
    assert!(captured.log.removed());
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn a_write_error_with_the_channel_full_ends_the_reader_and_removes_the_interface() {
    let (captured, _, dropped) = capture_over(
        FULL,
        STREAM_DEPTH as u64,
        true,
        Sink {
            fail_at: Some(1),
            ..sink()
        },
    );
    let error = captured.result.unwrap_err();
    assert!(error.contains("the client is gone"), "{error}");
    assert!(!error.contains(" 0 discarded"), "{error}");
    assert!(captured.sink.bytes.is_empty());
    assert!(captured.log.removed());
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn an_exit_other_than_success_fails_a_cancelled_capture() {
    let (captured, _, dropped) = capture_over(
        Script {
            chunks: 2,
            closes: true,
            fail_after: None,
        },
        2,
        false,
        Sink {
            cancel_after: Some(0),
            ..sink()
        },
    );
    let error = captured.result.unwrap_err();
    assert!(error.contains("exited with 1"), "{error}");
    assert!(captured.log.removed());
    assert!(dropped.load(Ordering::SeqCst));
}

/// A capture whose stderr fails after its readiness, as `end` scripts:
/// it fails with the stderr's failure and the prefix read, after every
/// teardown step (the exit, the reader's join, the interface's removal).
fn a_failed_stderr_fails_the_capture(end: StderrEnd, failure: &str) {
    let (captured, _, dropped) = capture_with_stderr(
        Script {
            chunks: 2,
            closes: true,
            fail_after: None,
        },
        2,
        true,
        Sink {
            cancel_after: Some(2),
            ..sink()
        },
        Some(end),
    );
    let error = captured.result.unwrap_err();
    assert!(error.contains("the capture ended: cancelled"), "{error}");
    assert!(error.contains("exit: exited (0)"), "{error}");
    assert!(error.contains("interface removal: confirmed"), "{error}");
    assert!(
        error.contains("diagnostics: oer-capture-pid 7; the diagnostics' read failed: "),
        "{error}"
    );
    assert!(error.contains(failure), "{error}");
    assert!(captured.log.removed());
    assert!(dropped.load(Ordering::SeqCst));
    // Every byte of the stream still reached the sink.
    assert_eq!(captured.sink.bytes.len(), 2 * CHUNK + TAIL.len());
}

#[test]
fn a_stderr_read_error_after_readiness_fails_a_cancelled_capture() {
    a_failed_stderr_fails_the_capture(StderrEnd::ReadError, "cannot read router stderr");
}

#[test]
fn a_stderr_reader_panic_after_readiness_fails_a_cancelled_capture() {
    a_failed_stderr_fails_the_capture(StderrEnd::Panic, "the capture's stderr reader panicked");
}

#[test]
fn a_whole_stderr_leaves_a_cancelled_capture_successful() {
    // The control: the same capture, its stderr ending without a failure.
    let (captured, _, _) = capture_over(
        Script {
            chunks: 2,
            closes: true,
            fail_after: None,
        },
        2,
        true,
        Sink {
            cancel_after: Some(2),
            ..sink()
        },
    );
    let report = captured.result.unwrap();
    assert_eq!(report.bytes, (2 * CHUNK + TAIL.len()) as u64);
}

#[test]
fn a_read_error_ends_the_stream_and_fails_the_capture() {
    let (captured, _, dropped) = capture_over(
        Script {
            chunks: 4,
            closes: true,
            fail_after: Some(2),
        },
        0,
        true,
        sink(),
    );
    let error = captured.result.unwrap_err();
    assert!(
        error.contains("ended by itself") && error.contains("ssh died"),
        "{error}"
    );
    assert!(captured.log.removed());
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn a_stream_that_does_not_end_after_the_stop_times_out_its_drain_and_is_still_joined() {
    // The writer holds the pipe open and silent after the stop signal.
    let (captured, _, dropped) = capture_over(
        Script {
            chunks: 2,
            closes: false,
            fail_after: None,
        },
        2,
        true,
        Sink {
            cancel_after: Some(0),
            ..sink()
        },
    );
    let error = captured.result.unwrap_err();
    assert!(error.contains("did not end within the bound"), "{error}");
    assert!(captured.log.removed());
    assert!(dropped.load(Ordering::SeqCst));
}
