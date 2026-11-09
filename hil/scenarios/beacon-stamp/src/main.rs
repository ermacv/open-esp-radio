//! TEMPORARY experiment host program (branch `probe/beacon-timestamp`,
//! never merged): who writes the Timestamp of a transmitted beacon.
//!
//! `run <board> <bundle-directory> <console-log>` builds nothing: the
//! probe's bundle is built first by the standard
//! `cargo fw build esp32s31-beacon-stamp`, outside any lease, and its
//! identity comes from that build's own records (`bundle.rs`). Under one
//! lease on the board and the air it writes the bundle and reads the
//! probe's console from its reset to its `OER-RESULT:` line into the log.
//! The probe needs no network, peer or secret: its frames go out open, to
//! the broadcast address or to an absent receiver.
//!
//! The capture is not this program's: the stand's monitor interface
//! records the probe's channel (13) with radiotap TSFT into a pcap file
//! from before the probe's reset until after its result line.
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
mod result;

use std::{path::Path, process::ExitCode, time::Duration};

use oer_tools_lab_arbiter::{
    claims::{AIR, Claim},
    journal::ImageIdentity,
};
use oer_tools_lab_client::{
    client::Client,
    protocol::{FlashVia, LeaseRequest, Operation},
};

use crate::{bundle::Identity, console::ConsoleSession, result::ProbeResult};

const IMAGE: &str = "esp32s31-beacon-stamp";
/// From the probe's reset to its result line: boot, RF start, the clock
/// separation, 96 rounds of 100 ms and the dump.
const CONSOLE_TIMEOUT: Duration = Duration::from_secs(60);

type Result<T> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["run", board, bundle, console] => {
            match run(board, Path::new(bundle), Path::new(console)) {
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
                "usage: oer-hil-scenarios-beacon-stamp run <board> <bundle-directory> <console-log>\n       oer-hil-scenarios-beacon-stamp decide <console-log> <capture.pcap>"
            );
            ExitCode::from(2)
        }
    }
}

/// `Ok(true)` when the probe printed its pass line.
fn run(board_id: &str, bundle: &Path, console: &Path) -> Result<bool> {
    let identity = bundle::identity(bundle)?;
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
            claims: vec![Claim::board(&board.mac), Claim::exclusive(AIR)],
            maintenance: None,
            divisible: false,
        },
        &mut |message| eprintln!("beacon-stamp: {message}"),
    )?;
    eprintln!("beacon-stamp: lease {} granted", granted.lease);
    let ran = leased(&mut client, &board.mac, &owner, bundle, &identity, console);
    let released = client.release();
    eprintln!("beacon-stamp: lease {} released", granted.lease);
    let passed = ran?;
    released?;
    Ok(passed)
}

fn leased(
    client: &mut Client,
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
        "beacon-stamp: wrote the probe bundle, application {}; reading the probe from its reset",
        identity.application_sha256
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
        decided &= verdict.outcome.decided();
    }
    Ok(decided)
}
