//! TEMPORARY experiment host program (branch `probe/beacon-timestamp`,
//! never merged): who writes the Timestamp of a transmitted beacon.
//!
//! `run <board> <bundle-directory> <console-log> <capture.pcap>` builds
//! nothing: the probe's bundle is built first by the standard
//! `cargo fw build esp32s31-beacon-stamp`, outside any lease, and its
//! identity comes from that build's own records (`bundle.rs`). Under one
//! lease on the board, the router and the air it starts the router's
//! monitor capture of the probe's channel (`sniffer.rs`), writes the
//! bundle, lets the preflight (`preflight.rs`) accept the capture of the
//! probe's first boot, reads the probe's console from its reset to its
//! `OER-RESULT:` line into the log, stops the capture and releases the
//! lease; the three ends are kept and recorded apart, in
//! `<console-log>.outcome.json`. Every path is absolute and the bundle's
//! identity is read before the lease. The probe needs no network, peer or secret:
//! its frames go out open, to the broadcast address or to an absent
//! receiver.
//!
//! `decide <console-log> <capture.pcap>` reads both and applies the rule
//! of `decide.rs` to each kind of subject frame. It exits 0 when every
//! kind is decided, 2 when one is not or an input is refused.
#![forbid(unsafe_code)]

mod bundle;
mod capture;
mod console;
mod decide;
mod device;
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
        ["run", board, bundle, console, capture] => {
            match run(
                board,
                Path::new(bundle),
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
                "usage: oer-hil-scenarios-beacon-stamp run <board> <bundle-directory> <console-log> <capture.pcap>\n       oer-hil-scenarios-beacon-stamp decide <console-log> <capture.pcap>"
            );
            ExitCode::from(2)
        }
    }
}

/// `Ok(true)` when the probe printed its pass line, the capture stopped
/// and the lease was released.
fn run(board_id: &str, bundle: &Path, console: &Path, capture: &Path) -> Result<bool> {
    let Paths {
        bundle,
        console,
        capture,
        identity,
    } = paths(bundle, console, capture)?;
    eprintln!(
        "beacon-stamp: bundle of {} at {} (dirty {}), application {}",
        bundle::PACKAGE,
        identity.commit,
        identity.dirty,
        identity.application_sha256
    );
    let stand = oer_tools_lab_model::path()
        .and_then(|path| oer_tools_lab_model::Stand::load(&path))
        .map_err(|error| format!("the stand file: {error}"))?;
    let board = stand
        .board(board_id)
        .ok_or_else(|| format!("the stand file has no board `{board_id}`"))?;
    let owner = oer_tools_lab_arbiter::owner::from_environment()
        .map_err(|error| error.to_string())?
        .to_string();
    let mut client = Client::open("beacon-stamp")?;
    let granted = client.acquire(
        LeaseRequest {
            owner: owner.clone(),
            work: format!("beacon-stamp probe on {board_id}"),
            scenarios: Vec::new(),
            run: None,
            claims: vec![
                Claim::board(&board.mac),
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
    let captured = sniffer::with_capture(
        &mut client,
        &mut monitor,
        CHANNEL,
        &capture,
        |client, monitor| {
            leased(
                client, monitor, &board.mac, &owner, &bundle, &identity, &console,
            )
        },
    );
    let released = client.release();
    eprintln!("beacon-stamp: lease {} released", granted.lease);
    let (result, record) = ended(captured, released);
    // Recorded apart: the run's, the capture's and the lease's ends.
    let mut file = console.into_os_string();
    file.push(".outcome.json");
    let written = std::fs::write(&file, record.to_string())
        .map_err(|error| format!("{}: {error}", Path::new(&file).display()));
    match (result, written) {
        (result, Ok(())) => result,
        (Ok(_), Err(error)) => Err(error),
        (Err(result), Err(error)) => Err(format!("{result}; {error}")),
    }
}

/// The run's files, every path absolute, and the bundle's identity.
struct Paths {
    bundle: std::path::PathBuf,
    console: std::path::PathBuf,
    capture: std::path::PathBuf,
    identity: Identity,
}

/// Resolve and check the run's files before the lease: the LAB service
/// resolves a relative path from its own directory, not this one, so the
/// bundle goes to it absolute, existing, a directory whose records give
/// its identity.
fn paths(bundle: &Path, console: &Path, capture: &Path) -> Result<Paths> {
    let bundle = std::fs::canonicalize(bundle)
        .map_err(|error| format!("the bundle {}: {error}", bundle.display()))?;
    if !bundle.is_dir() {
        return Err(format!(
            "the bundle {} is not a directory",
            bundle.display()
        ));
    }
    let identity = bundle::identity(&bundle)?;
    let console = std::path::absolute(console)
        .map_err(|error| format!("the console log {}: {error}", console.display()))?;
    let capture = std::path::absolute(capture)
        .map_err(|error| format!("the capture {}: {error}", capture.display()))?;
    Ok(Paths {
        bundle,
        console,
        capture,
        identity,
    })
}

/// The run's result only when the probe ran, the capture stopped and the
/// lease was released; otherwise every failure, together. Beside it the
/// record of the three ends, each its own.
fn ended(
    captured: sniffer::Captured<bool>,
    released: Result<()>,
) -> (Result<bool>, serde_json::Value) {
    let probe = match captured.body {
        None => Err("the probe was not run".to_owned()),
        Some(result) => result,
    };
    let end = |ok: bool, detail: String| serde_json::json!({ "ok": ok, "detail": detail });
    let record = serde_json::json!({
        "probe": match &probe {
            Ok(passed) => end(true, format!("ran, pass {passed}")),
            Err(error) => end(false, error.clone()),
        },
        "capture": match &captured.capture {
            Ok(file) => end(true, file.display().to_string()),
            Err(error) => end(false, error.clone()),
        },
        "release": match &released {
            Ok(()) => end(true, "released".to_owned()),
            Err(error) => end(false, error.clone()),
        },
    });
    let result = match (probe, captured.capture, released) {
        (Ok(passed), Ok(file), Ok(())) => {
            eprintln!("beacon-stamp: capture {}", file.display());
            Ok(passed)
        }
        (probe, capture, released) => Err(format!(
            "probe: {}; capture: {}; release: {}",
            probe.map_or_else(|e| e, |passed| format!("ran, pass {passed}")),
            capture.map_or_else(|e| e, |file| format!("stopped, {}", file.display())),
            released.map_or_else(|e| e, |()| "released".to_owned()),
        )),
    };
    (result, record)
}

fn leased(
    client: &mut Client,
    monitor: &mut sniffer::RouterMonitor,
    mac: &str,
    owner: &str,
    bundle: &Path,
    identity: &Identity,
    console: &Path,
) -> Result<bool> {
    client.run(
        Operation::Flash {
            board: mac.to_owned(),
            bundle: bundle.display().to_string(),
            image: ImageIdentity {
                name: IMAGE.to_owned(),
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
    eprintln!(
        "beacon-stamp: wrote the probe bundle, application {}; preflight on its first boot",
        identity.application_sha256
    );
    let frames = preflight::wait::<Client, _>(monitor)?;
    eprintln!(
        "beacon-stamp: preflight accepted {frames} probe frames with TSFT; reading the probe from its reset"
    );
    let mut log = std::fs::File::create(console)
        .map_err(|error| format!("{}: {error}", console.display()))?;
    let running = client.start(Operation::Console {
        board: mac.to_owned(),
        reset: true,
    })?;
    let cancel = running.input();
    let session = ConsoleSession::new(running, move || {
        let _ = cancel.cancel();
    });
    let verdict = session.result(&mut log, CONSOLE_TIMEOUT);
    session.finish();
    match verdict? {
        ProbeResult::Pass => Ok(true),
        ProbeResult::Fail(reason) => {
            eprintln!("beacon-stamp: probe result: fail {reason}");
            Ok(false)
        }
    }
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

    #[test]
    fn only_a_run_whose_capture_stopped_and_lease_ended_has_a_result() {
        let captured = sniffer::Captured {
            body: Some(Ok(true)),
            capture: Ok(PathBuf::from("run.pcap")),
        };
        let (result, record) = ended(captured, Ok(()));
        assert_eq!(result, Ok(true));
        assert_eq!(record["probe"]["ok"], true);
        assert_eq!(record["capture"]["ok"], true);
        assert_eq!(record["release"]["ok"], true);
        let captured = sniffer::Captured {
            body: Some(Ok(true)),
            capture: Ok(PathBuf::from("run.pcap")),
        };
        let (result, record) = ended(captured, Err("lab gone".into()));
        assert!(result.unwrap_err().contains("lab gone"));
        assert_eq!(record["probe"]["ok"], true);
        assert_eq!(record["release"]["detail"], "lab gone");
    }

    #[test]
    fn the_bundle_is_absolute_existing_and_identified_before_the_lease() {
        let directory = tempfile::tempdir().unwrap();
        let bundle = directory.path().join("bundle");
        assert!(paths(&bundle, Path::new("c.log"), Path::new("c.pcap")).is_err());
        std::fs::write(&bundle, b"not a directory").unwrap();
        assert!(paths(&bundle, Path::new("c.log"), Path::new("c.pcap")).is_err());
        std::fs::remove_file(&bundle).unwrap();
        std::fs::create_dir(&bundle).unwrap();
        // A directory without the build's records has no identity.
        assert!(paths(&bundle, Path::new("c.log"), Path::new("c.pcap")).is_err());
        let application = b"probe";
        std::fs::write(bundle.join("application.bin"), application).unwrap();
        let sha256 = oer_tools_os_durable::sha256_bytes(application);
        std::fs::write(
            bundle.join("bundle.json"),
            serde_json::json!({"flash_files": [{"file": "application.bin", "sha256": sha256, "bytes": 5}]})
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            bundle.join("sizes.json"),
            serde_json::json!({
                "provenance": {"commit": "eae701e4f", "dirty": false, "package": bundle::PACKAGE},
                "image_files": {"application.bin": 5},
            })
            .to_string(),
        )
        .unwrap();
        let resolved = paths(&bundle, Path::new("c.log"), Path::new("c.pcap")).unwrap();
        assert!(resolved.bundle.is_absolute() && resolved.console.is_absolute());
        assert!(resolved.capture.is_absolute());
        assert_eq!(resolved.identity.commit, "eae701e4f");
    }

    #[test]
    fn every_failure_is_kept_together() {
        let captured = sniffer::Captured::<bool> {
            body: Some(Err("console timeout".into())),
            capture: Err("tcpdump lost".into()),
        };
        let (result, record) = ended(captured, Err("release refused".into()));
        let error = result.unwrap_err();
        for cause in ["console timeout", "tcpdump lost", "release refused"] {
            assert!(error.contains(cause), "{error}");
        }
        assert_eq!(record["probe"]["detail"], "console timeout");
        assert_eq!(record["capture"]["detail"], "tcpdump lost");
        assert_eq!(record["release"]["detail"], "release refused");
        let captured = sniffer::Captured::<bool> {
            body: None,
            capture: Err("the capture did not start".into()),
        };
        let error = ended(captured, Ok(())).0.unwrap_err();
        assert!(error.contains("not run") && error.contains("did not start"));
    }
}
