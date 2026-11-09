//! TEMPORARY experiment host program (branch `probe/beacon-timestamp`,
//! never merged): who writes the Timestamp of a transmitted beacon.
//!
//! `run <board> <responder-board> <bundle> <responder-bundle> <console-log>
//! <capture.pcap>` builds nothing: both bundles are built first by the
//! standard `cargo fw build esp32s31-beacon-stamp` and `cargo fw build
//! esp32s31-ack-responder`, outside any lease, and their identities come
//! from those builds' own records (`bundle.rs`). Every path is made absolute
//! and both bundles are identified before the lease. Under one lease on both
//! boards, the router and the air:
//!
//! 1. the router's monitor capture of the probe's channel starts
//!    (`sniffer.rs`);
//! 2. the responder is written and read from its reset until it listens;
//! 3. the probe is written; the preflight (`preflight.rs`) accepts the
//!    capture of its first boot and decides the latch calibration
//!    (`latch.rs`) from that boot's calibration frames and the responder's
//!    ACKs;
//! 4. the probe is reset and read to its `OER-RESULT:` line;
//! 5. the responder is read to its own result line;
//! 6. the capture stops, and the lease is released.
//!
//! The probe's, the responder's, the preflight's, the capture's and the
//! lease's ends are kept and recorded apart, in `<console-log>.outcome.json`;
//! the responder's console goes to `<console-log>.responder.log`. The run
//! needs no network, peer or secret: the frames go out open.
//!
//! `decide <console-log> <capture.pcap>` reads both and applies the rule
//! of `decide.rs` to each kind of subject frame. It exits 0 when every
//! kind is decided, 2 when one is not or an input is refused.
#![forbid(unsafe_code)]

mod airtime;
mod bundle;
mod capture;
mod console;
mod decide;
mod device;
mod latch;
mod preflight;
mod result;
mod sniffer;

use std::{path::Path, process::ExitCode, time::Duration};

use oer_tools_lab_arbiter::{
    claims::{AIR, Claim},
    journal::ImageIdentity,
    readiness::ROUTER,
};
use oer_tools_lab_client::{
    client::Client,
    protocol::{FlashVia, LeaseRequest, Operation},
};

use crate::{bundle::Identity, console::ConsoleSession, result::ProbeResult};

const IMAGE: &str = "esp32s31-beacon-stamp";
const RESPONDER_IMAGE: &str = "esp32s31-ack-responder";
/// From the responder's reset to its listening line: boot and RF start.
const LISTEN_TIMEOUT: Duration = Duration::from_secs(30);
/// From the probe's result line to the responder's: its 120-s window from
/// its listening line, with margin.
const RESPONDER_TIMEOUT: Duration = Duration::from_secs(150);
/// The probe's channel (`hil/probes/esp32s31-beacon-stamp/src/image.rs`).
const CHANNEL: u8 = 13;
/// From the probe's reset to its result line: boot, RF start, the clock
/// separation, 96 rounds of 100 ms and the dump.
const CONSOLE_TIMEOUT: Duration = Duration::from_secs(60);

type Result<T> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    match words.as_slice() {
        [
            "run",
            board,
            responder,
            bundle,
            responder_bundle,
            console,
            capture,
        ] => {
            match run(
                Boards { board, responder },
                Path::new(bundle),
                Path::new(responder_bundle),
                Path::new(console),
                Path::new(capture),
            ) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => ExitCode::from(1),
                Err(error) => {
                    eprintln!("beacon-stamp: {error}");
                    ExitCode::from(2)
                }
            }
        }
        ["decide", console, capture] => match decide(Path::new(console), Path::new(capture)) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::from(2),
            Err(error) => {
                eprintln!("beacon-stamp: {error}");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!(
                "usage: oer-hil-scenarios-beacon-stamp run <board> <responder-board> <bundle-directory> <responder-bundle-directory> <console-log> <capture.pcap>\n       oer-hil-scenarios-beacon-stamp decide <console-log> <capture.pcap>"
            );
            ExitCode::from(2)
        }
    }
}

/// The two boards by their stand names.
struct Boards<'a> {
    board: &'a str,
    responder: &'a str,
}

/// `Ok(true)` when the probe printed its pass line, the responder its own,
/// the capture stopped and the lease was released.
fn run(
    boards: Boards<'_>,
    bundle: &Path,
    responder_bundle: &Path,
    console: &Path,
    capture: &Path,
) -> Result<bool> {
    let paths = paths(bundle, responder_bundle, console, capture)?;
    for (package, identity) in [
        (bundle::PACKAGE, &paths.identity),
        (bundle::RESPONDER_PACKAGE, &paths.responder_identity),
    ] {
        eprintln!(
            "beacon-stamp: bundle of {package} at {} (dirty {}), application {}",
            identity.commit, identity.dirty, identity.application_sha256
        );
    }
    let stand = oer_tools_lab_model::path()
        .and_then(|path| oer_tools_lab_model::Stand::load(&path))
        .map_err(|error| format!("the stand file: {error}"))?;
    let board = stand
        .board(boards.board)
        .ok_or_else(|| format!("the stand file has no board `{}`", boards.board))?;
    let responder = stand
        .board(boards.responder)
        .ok_or_else(|| format!("the stand file has no board `{}`", boards.responder))?;
    if board.mac == responder.mac {
        return Err("the probe and the responder need two boards".into());
    }
    let owner = oer_tools_lab_arbiter::owner::from_environment()
        .map_err(|error| error.to_string())?
        .to_string();
    let mut client = Client::open("beacon-stamp")?;
    let granted = client.acquire(
        LeaseRequest {
            owner: owner.clone(),
            work: format!(
                "beacon-stamp probe on {} with its responder on {}",
                boards.board, boards.responder
            ),
            scenarios: Vec::new(),
            run: None,
            claims: vec![
                Claim::board(&board.mac),
                Claim::board(&responder.mac),
                Claim::exclusive(ROUTER),
                Claim::exclusive(AIR),
            ],
            maintenance: None,
            divisible: false,
        },
        &mut |message| eprintln!("beacon-stamp: {message}"),
    )?;
    eprintln!("beacon-stamp: lease {} granted", granted.lease);
    let mut monitor = sniffer::RouterMonitor::new();
    let macs = Macs {
        probe: &board.mac,
        responder: &responder.mac,
    };
    let captured = sniffer::with_capture(
        &mut client,
        &mut monitor,
        CHANNEL,
        &paths.capture,
        |client, monitor| leased(client, monitor, &macs, &owner, &paths),
    );
    let released = client.release();
    eprintln!("beacon-stamp: lease {} released", granted.lease);
    let (result, record) = ended(captured, released);
    // Recorded apart: every end of the run.
    let mut file = paths.console.clone().into_os_string();
    file.push(".outcome.json");
    let written = std::fs::write(&file, record.to_string())
        .map_err(|error| format!("{}: {error}", Path::new(&file).display()));
    match (result, written) {
        (result, Ok(())) => result,
        (Ok(_), Err(error)) => Err(error),
        (Err(result), Err(error)) => Err(format!("{result}; {error}")),
    }
}

/// The run's files, every path absolute, and both bundles' identities.
struct Paths {
    bundle: std::path::PathBuf,
    responder: std::path::PathBuf,
    console: std::path::PathBuf,
    capture: std::path::PathBuf,
    identity: Identity,
    responder_identity: Identity,
}

/// A bundle path made absolute, checked to be a directory and identified
/// as `package`'s.
fn bundle_of(path: &Path, package: &str) -> Result<(std::path::PathBuf, Identity)> {
    let path = std::fs::canonicalize(path)
        .map_err(|error| format!("the bundle {}: {error}", path.display()))?;
    if !path.is_dir() {
        return Err(format!("the bundle {} is not a directory", path.display()));
    }
    let identity = bundle::identity(&path, package)?;
    Ok((path, identity))
}

/// Resolve and check the run's files before the lease: the LAB service
/// resolves a relative path from its own directory, not this one, so each
/// bundle goes to it absolute, existing, a directory whose records give
/// its identity.
fn paths(bundle: &Path, responder: &Path, console: &Path, capture: &Path) -> Result<Paths> {
    let (bundle, identity) = bundle_of(bundle, bundle::PACKAGE)?;
    let (responder, responder_identity) = bundle_of(responder, bundle::RESPONDER_PACKAGE)?;
    let console = std::path::absolute(console)
        .map_err(|error| format!("the console log {}: {error}", console.display()))?;
    let capture = std::path::absolute(capture)
        .map_err(|error| format!("the capture {}: {error}", capture.display()))?;
    Ok(Paths {
        bundle,
        responder,
        console,
        capture,
        identity,
        responder_identity,
    })
}

/// What the lease's body left: the probe's result, the responder's, and
/// the preflight's latch calibration.
#[derive(Debug, Eq, PartialEq)]
struct Leased {
    probe: bool,
    responder: Result<bool>,
    latch: String,
}

/// The run's result only when the probe ran, the responder ended, the
/// capture stopped and the lease was released; otherwise every failure,
/// together. Beside it the record of each end, each its own.
fn ended(
    captured: sniffer::Captured<Leased>,
    released: Result<()>,
) -> (Result<bool>, serde_json::Value) {
    let body = match captured.body {
        None => Err("the probe was not run".to_owned()),
        Some(result) => result,
    };
    let end = |ok: bool, detail: String| serde_json::json!({ "ok": ok, "detail": detail });
    let (probe, responder, latch) = match &body {
        Ok(leased) => (
            end(true, format!("ran, pass {}", leased.probe)),
            match &leased.responder {
                Ok(passed) => end(true, format!("ran, pass {passed}")),
                Err(error) => end(false, error.clone()),
            },
            end(true, leased.latch.clone()),
        ),
        Err(error) => (
            end(false, error.clone()),
            end(false, "not read".to_owned()),
            end(false, "not decided".to_owned()),
        ),
    };
    let record = serde_json::json!({
        "probe": probe,
        "responder": responder,
        "latch": latch,
        "capture": match &captured.capture {
            Ok(file) => end(true, file.display().to_string()),
            Err(error) => end(false, error.clone()),
        },
        "release": match &released {
            Ok(()) => end(true, "released".to_owned()),
            Err(error) => end(false, error.clone()),
        },
    });
    let result = match (body, captured.capture, released) {
        (
            Ok(Leased {
                probe,
                responder: Ok(responder),
                ..
            }),
            Ok(file),
            Ok(()),
        ) => {
            eprintln!("beacon-stamp: capture {}", file.display());
            Ok(probe && responder)
        }
        (body, capture, released) => Err(format!(
            "probe: {}; capture: {}; release: {}",
            body.map_or_else(
                |e| e,
                |leased| format!(
                    "ran, pass {}, responder {:?}",
                    leased.probe, leased.responder
                )
            ),
            capture.map_or_else(|e| e, |file| format!("stopped, {}", file.display())),
            released.map_or_else(|e| e, |()| "released".to_owned()),
        )),
    };
    (result, record)
}

struct Macs<'a> {
    probe: &'a str,
    responder: &'a str,
}

fn flash(
    client: &mut Client,
    mac: &str,
    owner: &str,
    image: &str,
    bundle: &Path,
    identity: &Identity,
) -> Result<()> {
    client.run(
        Operation::Flash {
            board: mac.to_owned(),
            bundle: bundle.display().to_string(),
            image: ImageIdentity {
                name: image.to_owned(),
                sha256: identity.application_sha256.clone(),
                commit: Some(identity.commit.clone()),
                dirty: Some(identity.dirty),
                origin: std::env::args().collect::<Vec<_>>().join(" "),
            },
            owner: owner.to_owned(),
            via: FlashVia::Usb,
            skip_carried: false,
        },
        &mut std::io::sink(),
        None,
    )?;
    Ok(())
}

/// A console session of `image` on board `mac` from its reset.
fn console_of(client: &mut Client, image: &'static str, mac: &str) -> Result<ConsoleSession> {
    let running = client.start(Operation::Console {
        board: mac.to_owned(),
        reset: true,
    })?;
    let cancel = running.input();
    Ok(ConsoleSession::new(image, running, move || {
        let _ = cancel.cancel();
    }))
}

fn passed(result: ProbeResult, image: &str) -> bool {
    match result {
        ProbeResult::Pass => true,
        ProbeResult::Fail(reason) => {
            eprintln!("beacon-stamp: {image} result: fail {reason}");
            false
        }
    }
}

fn leased(
    client: &mut Client,
    monitor: &mut sniffer::RouterMonitor,
    macs: &Macs<'_>,
    owner: &str,
    paths: &Paths,
) -> Result<Leased> {
    flash(
        client,
        macs.responder,
        owner,
        RESPONDER_IMAGE,
        &paths.responder,
        &paths.responder_identity,
    )?;
    let mut responder_file = paths.console.clone().into_os_string();
    responder_file.push(".responder.log");
    let mut responder_log = std::fs::File::create(&responder_file)
        .map_err(|error| format!("{}: {error}", Path::new(&responder_file).display()))?;
    // Ends with this function on every path: cancelled and joined.
    let responder = console_of(client, "ack-responder", macs.responder)?;
    responder.wait_for(&mut responder_log, "ACKRESP listening", LISTEN_TIMEOUT)?;
    eprintln!("beacon-stamp: the responder listens; writing the probe");
    flash(
        client,
        macs.probe,
        owner,
        IMAGE,
        &paths.bundle,
        &paths.identity,
    )?;
    let preflight = preflight::wait::<Client, _>(monitor)?;
    let latch = format!(
        "{:?} ({} pairs, {} unanswered, {} without an observed PHY, {} contradictory, {:?})",
        preflight.latch,
        preflight.pairs,
        preflight.unanswered,
        preflight.unobserved,
        preflight.contradictory,
        preflight.evidence
    );
    eprintln!(
        "beacon-stamp: preflight accepted {} probe frames with TSFT; latch calibration: {latch}; \
         reading the probe from its reset",
        preflight.frames
    );
    let mut log = std::fs::File::create(&paths.console)
        .map_err(|error| format!("{}: {error}", paths.console.display()))?;
    let session = console_of(client, "beacon-stamp", macs.probe)?;
    let verdict = session.result(&mut log, CONSOLE_TIMEOUT);
    session.finish();
    let probe = passed(verdict?, "beacon-stamp");
    let responded = responder
        .result(&mut responder_log, RESPONDER_TIMEOUT)
        .map(|result| passed(result, "ack-responder"));
    responder.finish();
    Ok(Leased {
        probe,
        responder: responded,
        latch,
    })
}

/// `Ok(true)` when every kind is decided.
fn decide(console: &Path, capture: &Path) -> Result<bool> {
    let log = std::fs::read_to_string(console)
        .map_err(|error| format!("{}: {error}", console.display()))?;
    let run = device::parse(&log)?;
    let bytes =
        std::fs::read(capture).map_err(|error| format!("{}: {error}", capture.display()))?;
    let capture = capture::parse(&bytes)?;
    println!(
        "run {:08x}: {} records, probe error {}; capture: {} frames, {} without TSFT, {} with a bad FCS",
        run.run,
        run.records.len(),
        run.error.as_deref().unwrap_or("-"),
        capture.frames.len(),
        capture.without_tsft,
        capture.bad_fcs
    );
    println!(
        "probe cleanup: mac {} dma {} final check {}; RX buffer overflows {}",
        run.stops.mac, run.stops.dma, run.stops.finish, run.rx_full
    );
    println!("conditions: {CONDITIONS}");
    let (clocks, verdicts) = decide::decide(&run, &capture);
    match &clocks {
        Ok(clocks) => println!(
            "clocks: AP TSF/local {:+.2} ppm; sniffer/local {:+.2} ppm (+-{:.2}); undelayed jitter {:.1} us",
            (clocks.tsf_rate - 1.0) * 1e6,
            (clocks.sniffer_rate - 1.0) * 1e6,
            clocks.sniffer_rate_uncertainty * 1e6,
            clocks.jitter_us
        ),
        Err(shortfall) => println!("clocks: {shortfall:?}"),
    }
    let mut decided = true;
    for verdict in &verdicts {
        let evidence = &verdict.evidence;
        println!(
            "{}: {}\n  matched {} (missing {}, duplicated {}), confirmed delayed {}, decisive pairs {}, least delay change {:.0} us, largest tolerance {:.1} us\n  unchanged octets {:08b}, worst residual at transmission {:.1} us, at handoff {:.1} us\n  read back after completion: K {}, the air's value {}, other {}",
            verdict.kind.name(),
            verdict.outcome.name(),
            evidence.matched,
            evidence.missing,
            evidence.duplicates,
            evidence.confirmed_delayed,
            evidence.decisive,
            evidence.min_delay_change_us,
            evidence.largest_tolerance_us,
            evidence.unchanged,
            evidence.worst_transmission_residual_us,
            evidence.worst_handoff_residual_us,
            evidence.readback_kept,
            evidence.readback_air,
            evidence.readback_other
        );
        if let Some(withheld) = &evidence.withheld {
            println!("  withheld by the run's condition: {}", withheld.name());
        }
        decided &= verdict.outcome.decided();
    }
    Ok(decided)
}

/// What every outcome is conditional on: it says nothing of the vendor
/// path beyond them.
const CONDITIONS: &str = "AP interface, DSSS 1 Mb/s, one attempt; AIFSN 2 and CW 0 on both queues (vendor VO: CW 3..7); \
the blocker on VI ahead of the VO subject, not in the subject's queue; probe responses unsolicited, to an absent receiver, \
without the vendor's TX-control bit 27 (no equivalent here): a probe-response outcome does not describe the vendor's \
unicast probe-response path; the sniffer's TSFT is the mt76 RX descriptor's 32-bit timestamp, which the driver flags as \
the first symbol of the MPDU and mac80211 passes on unchanged; what counter it reads is not established";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn leased(responder: Result<bool>) -> Leased {
        Leased {
            probe: true,
            responder,
            latch: "Latch(MpduStart)".into(),
        }
    }

    #[test]
    fn only_a_run_whose_responder_capture_and_lease_ended_has_a_result() {
        let captured = sniffer::Captured {
            body: Some(Ok(leased(Ok(true)))),
            capture: Ok(PathBuf::from("run.pcap")),
        };
        let (result, record) = ended(captured, Ok(()));
        assert_eq!(result, Ok(true));
        for end in ["probe", "responder", "latch", "capture", "release"] {
            assert_eq!(record[end]["ok"], true, "{end}");
        }
        let captured = sniffer::Captured {
            body: Some(Ok(leased(Ok(true)))),
            capture: Ok(PathBuf::from("run.pcap")),
        };
        let (result, record) = ended(captured, Err("lab gone".into()));
        assert!(result.unwrap_err().contains("lab gone"));
        assert_eq!(record["probe"]["ok"], true);
        assert_eq!(record["release"]["detail"], "lab gone");
        let captured = sniffer::Captured {
            body: Some(Ok(leased(Err("no responder result".into())))),
            capture: Ok(PathBuf::from("run.pcap")),
        };
        let (result, record) = ended(captured, Ok(()));
        assert!(result.unwrap_err().contains("no responder result"));
        assert_eq!(record["probe"]["ok"], true);
        assert_eq!(record["responder"]["detail"], "no responder result");
    }

    /// A bundle directory whose records name `package`.
    fn bundle_dir(root: &Path, name: &str, package: &str) -> PathBuf {
        let bundle = root.join(name);
        std::fs::create_dir(&bundle).unwrap();
        let application = name.as_bytes();
        std::fs::write(bundle.join("application.bin"), application).unwrap();
        let sha256 = oer_tools_os_durable::sha256_bytes(application);
        std::fs::write(
            bundle.join("bundle.json"),
            serde_json::json!({"flash_files": [{"file": "application.bin", "sha256": sha256, "bytes": application.len()}]})
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            bundle.join("sizes.json"),
            serde_json::json!({
                "provenance": {"commit": "61564b55f", "dirty": false, "package": package},
                "image_files": {"application.bin": application.len()},
            })
            .to_string(),
        )
        .unwrap();
        bundle
    }

    #[test]
    fn both_bundles_are_absolute_existing_and_identified_before_the_lease() {
        let directory = tempfile::tempdir().unwrap();
        let (log, pcap) = (Path::new("c.log"), Path::new("c.pcap"));
        let missing = directory.path().join("missing");
        let probe = bundle_dir(directory.path(), "probe", bundle::PACKAGE);
        let responder = bundle_dir(directory.path(), "responder", bundle::RESPONDER_PACKAGE);
        assert!(paths(&missing, &responder, log, pcap).is_err());
        assert!(paths(&probe, &missing, log, pcap).is_err());
        let file = directory.path().join("file");
        std::fs::write(&file, b"not a directory").unwrap();
        assert!(paths(&file, &responder, log, pcap).is_err());
        // Each bundle must be its own image's.
        assert!(paths(&responder, &probe, log, pcap).is_err());
        let empty = directory.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(paths(&probe, &empty, log, pcap).is_err());
        let resolved = paths(&probe, &responder, log, pcap).unwrap();
        assert!(resolved.bundle.is_absolute() && resolved.responder.is_absolute());
        assert!(resolved.console.is_absolute() && resolved.capture.is_absolute());
        assert_eq!(resolved.identity.commit, "61564b55f");
        assert_eq!(resolved.responder_identity.commit, "61564b55f");
    }

    #[test]
    fn every_failure_is_kept_together() {
        let captured = sniffer::Captured::<Leased> {
            body: Some(Err("console timeout".into())),
            capture: Err("tcpdump lost".into()),
        };
        let (result, record) = ended(captured, Err("release refused".into()));
        let error = result.unwrap_err();
        for cause in ["console timeout", "tcpdump lost", "release refused"] {
            assert!(error.contains(cause), "{error}");
        }
        assert_eq!(record["probe"]["detail"], "console timeout");
        assert_eq!(record["responder"]["ok"], false);
        assert_eq!(record["capture"]["detail"], "tcpdump lost");
        assert_eq!(record["release"]["detail"], "release refused");
        let captured = sniffer::Captured::<Leased> {
            body: None,
            capture: Err("the capture did not start".into()),
        };
        let error = ended(captured, Ok(())).0.unwrap_err();
        assert!(error.contains("not run") && error.contains("did not start"));
    }
}
