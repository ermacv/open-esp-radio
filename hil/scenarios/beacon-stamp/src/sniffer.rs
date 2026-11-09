//! The run's capture on the stand's sniffer, the mt76 router's monitor
//! interface, as part of the lease.
//!
//! Order within the lease: the capture starts before the probe is written,
//! so it holds the run from the probe's reset on; it stops after the
//! probe's result line or whatever ended the run early. A started capture
//! is stopped on every outcome, and the run's result, the capture's stop
//! and the lease's release are each kept: a failure of one never hides
//! another. At the lease's end the LAB service parks the router, which
//! removes a monitor interface a run added (`tools/lab/service/src/router.rs`,
//! `park`), whatever the run did.
//!
//! What a capture is worth is the decision's (`decide.rs`): every frame's
//! radiotap TSFT must follow the air. A TSFT from the router MAC's own TSF
//! does; a host receive time scatters and is refused there
//! (`J_SNIFFER_MAX_US`). That the mt76 driver reports its own TSF is not
//! confirmed yet.

use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, String>;

/// One capture's life on a sniffer.
pub trait Sniffer {
    /// Start capturing `channel` into `file`.
    fn start(&mut self, channel: u8, file: &Path) -> Result<()>;
    /// Stop the capture started last; the file it wrote.
    fn stop(&mut self) -> Result<PathBuf>;
}

/// What a captured run left: the body's result (`None`: it did not run,
/// the capture never started) and the capture's.
#[derive(Debug, Eq, PartialEq)]
pub struct Captured<T> {
    pub body: Option<Result<T>>,
    pub capture: Result<PathBuf>,
}

/// Run `body` under a capture of `channel` into `file`: a refused start
/// runs nothing; a started capture is stopped whatever `body` returned.
pub fn with_capture<S: Sniffer, T>(
    sniffer: &mut S,
    channel: u8,
    file: &Path,
    body: impl FnOnce() -> Result<T>,
) -> Captured<T> {
    if let Err(error) = sniffer.start(channel, file) {
        return Captured {
            body: None,
            capture: Err(format!("the capture did not start: {error}")),
        };
    }
    let body = body();
    Captured {
        body: Some(body),
        capture: sniffer.stop(),
    }
}

/// The stand's router as the sniffer.
///
/// The LAB service owns every access to the router, and it has no monitor
/// capture yet. The operation this program needs: on the router held
/// exclusively, create the monitor interface on its radio's PHY, tune it
/// to the channel, stream `tcpdump -w -` (classic pcap, radiotap) to the
/// client until the operation stops, then remove the interface; the
/// service journals the monitor so that its park removes it on every
/// outcome. Until the service has it, the capture refuses to start and
/// the run writes nothing to the board.
pub struct RouterMonitor;

impl Sniffer for RouterMonitor {
    fn start(&mut self, _channel: u8, _file: &Path) -> Result<()> {
        Err("the LAB service has no router monitor-capture operation".into())
    }

    fn stop(&mut self) -> Result<PathBuf> {
        Err("no router capture was started".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sniffer that refuses its start or its stop as told and records
    /// every call.
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

    impl Sniffer for Fake {
        fn start(&mut self, channel: u8, _: &Path) -> Result<()> {
            assert_eq!(channel, 13);
            self.calls.push("start");
            if self.refuse_start {
                Err("no monitor".into())
            } else {
                Ok(())
            }
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

    #[test]
    fn a_refused_start_runs_nothing() {
        let mut sniffer = Fake::new();
        sniffer.refuse_start = true;
        let mut ran = false;
        let captured = with_capture(&mut sniffer, 13, Path::new("run.pcap"), || {
            ran = true;
            Ok(())
        });
        assert!(!ran);
        assert_eq!(captured.body, None);
        assert!(captured.capture.unwrap_err().contains("no monitor"));
        assert_eq!(sniffer.calls, ["start"]);
    }

    #[test]
    fn a_started_capture_is_stopped_after_a_completed_run() {
        let mut sniffer = Fake::new();
        let captured = with_capture(&mut sniffer, 13, Path::new("run.pcap"), || Ok(true));
        assert_eq!(captured.body, Some(Ok(true)));
        assert_eq!(captured.capture, Ok(PathBuf::from("run.pcap")));
        assert_eq!(sniffer.calls, ["start", "stop"]);
    }

    #[test]
    fn a_failed_run_still_stops_the_capture() {
        let mut sniffer = Fake::new();
        let captured: Captured<bool> =
            with_capture(&mut sniffer, 13, Path::new("run.pcap"), || {
                Err("console timeout".into())
            });
        assert_eq!(captured.body, Some(Err("console timeout".into())));
        assert!(captured.capture.is_ok());
        assert_eq!(sniffer.calls, ["start", "stop"]);
    }

    #[test]
    fn a_failed_run_and_a_failed_stop_keep_both_causes() {
        let mut sniffer = Fake::new();
        sniffer.refuse_stop = true;
        let captured: Captured<bool> =
            with_capture(&mut sniffer, 13, Path::new("run.pcap"), || {
                Err("flash refused".into())
            });
        assert_eq!(captured.body, Some(Err("flash refused".into())));
        assert_eq!(captured.capture, Err("tcpdump lost".into()));
        assert_eq!(sniffer.calls, ["start", "stop"]);
    }

    #[test]
    fn the_router_refuses_until_the_lab_service_captures() {
        let mut router = RouterMonitor;
        let mut ran = false;
        let captured = with_capture(&mut router, 13, Path::new("run.pcap"), || {
            ran = true;
            Ok(())
        });
        assert!(!ran);
        assert!(captured.capture.is_err());
    }
}
