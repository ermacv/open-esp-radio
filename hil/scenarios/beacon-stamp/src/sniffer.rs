//! The run's capture on the stand's sniffer, the mt76 router's monitor, as
//! part of the lease (`RouterOperation::MonitorCapture`, `tools/lab`).
//!
//! Within the lease the capture starts first; then the probe is written,
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

use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

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

/// The stand's router through the LAB service: the capture's stream goes
/// to the file by a thread of its own until the operation finishes.
pub struct RouterMonitor {
    running: Option<Running>,
}

struct Running {
    cancel: oer_tools_lab_client::client::Input,
    reader: std::thread::JoinHandle<Result<serde_json::Value>>,
    file: PathBuf,
}

impl RouterMonitor {
    pub const fn new() -> Self {
        Self { running: None }
    }
}

impl Sniffer<oer_tools_lab_client::client::Client> for RouterMonitor {
    fn start(
        &mut self,
        client: &mut oer_tools_lab_client::client::Client,
        channel: u8,
        file: &Path,
    ) -> Result<()> {
        use oer_tools_lab_client::{
            client::Event,
            protocol::{Operation, RouterOperation},
        };
        let mut output =
            std::fs::File::create(file).map_err(|error| format!("{}: {error}", file.display()))?;
        let mut running = client.start(Operation::Router {
            request: RouterOperation::MonitorCapture { channel },
        })?;
        let cancel = running.input();
        let reader = std::thread::spawn(move || {
            let mut written = Ok(());
            let mut lost = 0u64;
            loop {
                match running.event() {
                    Event::Output(bytes) => {
                        if written.is_ok() {
                            written = output
                                .write_all(&bytes)
                                .map_err(|error| format!("the capture file: {error}"));
                            if written.is_err() {
                                let _ = running.cancel();
                            }
                        }
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
        self.running = Some(Running {
            cancel,
            reader,
            file: file.to_owned(),
        });
        Ok(())
    }

    fn captured(&mut self) -> Result<Vec<u8>> {
        let running = self.running.as_ref().ok_or("no capture runs")?;
        std::fs::read(&running.file).map_err(|error| format!("{}: {error}", running.file.display()))
    }

    fn stop(&mut self) -> Result<PathBuf> {
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
}
