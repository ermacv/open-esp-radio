//! The commands of `cargo fw`, called by its thin binary (`tools/image/cli`).

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant, SystemTime},
};

use oer_lab_model as stand;

use oer_lab_arbiter::claims::Claim;
use oer_lab_client::client::{Client, Running};
use oer_lab_client::protocol::{FlashVia, LeaseRequest, Operation};

use crate::image;
use oer_image_build::{Check, Enforced, Result};

/// The chips whose boards this tool resets and writes. Another chip's board
/// is only listed: its reset lines may mean something else (an ESP32-C5's
/// RTS reset enters its download mode).
const SUPPORTED_CHIPS: [&str; 1] = [image::CHIP];

/// The marker an image prints with its verdict.
const RESULT_MARKER: &str = "OER-RESULT: ";

/// The repository this tool runs in.
pub fn root() -> Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    Ok(std::fs::canonicalize(manifest.join("../../.."))?)
}

/// The stand board `selector` names, which this tool may reset and write.
fn board(selector: &str) -> Result<stand::Board> {
    let path = stand::path()?;
    let stand = stand::Stand::load(&path)?;
    let board = stand.board(selector).cloned().ok_or_else(|| {
        format!(
            "the stand file {} has no board `{selector}`",
            path.display()
        )
    })?;
    if !SUPPORTED_CHIPS.contains(&board.chip.as_str()) {
        return Err(format!(
            "board {} is an {}: this tool resets and writes only {}",
            board.id,
            board.chip,
            SUPPORTED_CHIPS.join(", ")
        )
        .into());
    }
    Ok(board)
}

/// A lease of `board` and the air from the LAB service, which runs every
/// operation on it. A command that ends releases it and fails when the
/// service could not bring the board back to its end state (its ROM's
/// download mode or, for a board without a USB reset, its power off).
fn lease(board: &stand::Board) -> Result<Client> {
    let command = std::env::args().collect::<Vec<_>>().join(" ");
    let mut client = Client::open(&command)?;
    client.acquire(
        LeaseRequest {
            owner: oer_lab_arbiter::owner::from_environment()?.to_string(),
            work: command,
            scenarios: Vec::new(),
            run: None,
            // An image's radio works only on its command: the board alone.
            claims: vec![Claim::board(&board.mac)],
            maintenance: None,
            divisible: false,
        },
        &mut |message| eprintln!("fw: {message}"),
    )?;
    Ok(client)
}

/// Write `image`'s bundle into `board` through the service, which records
/// the board's receipt, journals the write and starts the image.
fn write_image(root: &Path, image: &str, board: &stand::Board, client: &mut Client) -> Result<()> {
    let bundle = image::bundle(root, image)?;
    eprintln!("fw: writing {image} to {}", board.id);
    let git = |arguments: &[&str]| oer_os_process::git::text(root, arguments).ok();
    let written = client.run(
        Operation::Flash {
            board: board.mac.clone(),
            bundle: bundle.directory().display().to_string(),
            image: oer_lab_arbiter::journal::ImageIdentity {
                name: image.to_owned(),
                sha256: String::new(),
                commit: git(&["rev-parse", "HEAD"]),
                dirty: git(&["status", "--porcelain"]).map(|status| !status.is_empty()),
                origin: std::env::args().collect::<Vec<_>>().join(" "),
            },
            owner: oer_lab_arbiter::owner::from_environment()?.to_string(),
            via: FlashVia::Usb,
            skip_carried: false,
        },
        &mut std::io::sink(),
        None,
    )?;
    let segments = written
        .pointer("/receipt/segments")
        .and_then(serde_json::Value::as_array)
        .ok_or("the service returned no receipt")?;
    for segment in segments {
        eprintln!(
            "fw:   {:<16} {:#08x} {:>8} bytes",
            segment["description"].as_str().unwrap_or("?"),
            segment["offset"].as_u64().unwrap_or_default(),
            segment["bytes"].as_u64().unwrap_or_default()
        );
    }
    Ok(())
}

/// The lines of a console stream, each without its line ending; the channel
/// ends with the stream.
fn lines(console: Running) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (send, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead as _;
        for line in std::io::BufReader::new(console).split(b'\n') {
            let Ok(mut line) = line else {
                break;
            };
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if send.send(line).is_err() {
                break;
            }
        }
    });
    lines
}

/// The next console line within `within`: `Ok(None)` when none came,
/// an error when the console closed.
fn next_line(
    lines: &std::sync::mpsc::Receiver<Vec<u8>>,
    within: Duration,
) -> std::result::Result<Option<Vec<u8>>, ()> {
    let deadline = Instant::now() + within;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(remaining) {
            // An empty message only keeps the reader alive.
            Ok(line) if line.is_empty() => {
                if remaining.is_zero() {
                    return Ok(None);
                }
            }
            Ok(line) => return Ok(Some(line)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Ok(None),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(()),
        }
    }
}

/// An image's verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Verdict {
    Pass,
    Fail,
}

/// The verdict a console line announces.
fn verdict(line: &str) -> Option<Verdict> {
    let rest = &line[line.find(RESULT_MARKER)? + RESULT_MARKER.len()..];
    match rest.split_whitespace().next()? {
        "pass" => Some(Verdict::Pass),
        "fail" => Some(Verdict::Fail),
        _ => None,
    }
}

fn console_verdict(raw: &[u8], text: &str) -> Option<Verdict> {
    // A retained panic message can itself contain the result marker.
    // Only a typed verdict closes the diagnostic sequence.
    if raw.starts_with(oer_diagnostics::PREFIX) && !text.starts_with(RESULT_MARKER) {
        return None;
    }
    verdict(text)
}

/// `YYYYMMDDTHHMMSSZ` of `time`.
fn timestamp(time: SystemTime) -> String {
    let seconds = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    // Gregorian dates repeat every 400 years. Reduce into jiff's supported
    // range without narrowing the existing u64 timestamp domain.
    const CYCLE_SECONDS: u64 = 146_097 * 86_400;
    let cycles = seconds / CYCLE_SECONDS;
    let within_cycle = (seconds % CYCLE_SECONDS) as i64;
    let date = jiff::Timestamp::from_second(within_cycle)
        .expect("a 400-year cycle from 1970 fits jiff's timestamp range")
        .to_zoned(jiff::tz::TimeZone::UTC);
    let year = date.year() as u64 + cycles * 400;
    format!(
        "{year:04}{:02}{:02}T{:02}{:02}{:02}Z",
        date.month(),
        date.day(),
        date.hour(),
        date.minute(),
        date.second()
    )
}

pub fn run(root: &Path, image: &str, selector: &str, timeout: Duration) -> Result<ExitCode> {
    let board = board(selector)?;
    image::build(root, image, Enforced::Reported, None)?;
    let mut client = lease(&board)?;
    write_image(root, image, &board, &mut client)?;
    let run = root
        .join("target/fw/runs")
        .join(format!("{}-{image}", timestamp(SystemTime::now())));
    std::fs::create_dir_all(&run)?;
    let log_path = run.join("log.txt");
    let mut log = std::fs::File::create(&log_path)?;
    let mut raw_log = std::fs::File::create(run.join("console.raw.txt"))?;
    let mut diagnostics = crate::diagnostics::Decoder::default();
    let console = client.start(Operation::Console {
        board: board.mac.clone(),
        reset: true,
    })?;
    let stop = console.input();
    let lines = lines(console);
    eprintln!(
        "fw: {} reset into {image}; reading its console for {}s",
        board.id,
        timeout.as_secs()
    );
    let deadline = Instant::now() + timeout;
    let verdict = loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break None;
        };
        match next_line(&lines, remaining) {
            Ok(Some(line)) => {
                raw_log.write_all(&line)?;
                raw_log.write_all(b"\n")?;
                let Some(text) = diagnostics.line(&line)? else {
                    continue;
                };
                println!("{text}");
                writeln!(log, "{text}")?;
                if let Some(verdict) = console_verdict(&line, &text) {
                    break Some(verdict);
                }
            }
            Ok(None) => break None,
            Err(()) => return Err("the board's console closed".into()),
        }
    };
    let _ = stop.cancel();
    drop(lines);
    eprintln!("fw: log {}", log_path.display());
    client.release()?;
    Ok(match verdict {
        Some(Verdict::Pass) => ExitCode::SUCCESS,
        Some(Verdict::Fail) => ExitCode::from(1),
        None => {
            eprintln!("fw: no {RESULT_MARKER}line within {}s", timeout.as_secs());
            ExitCode::from(2)
        }
    })
}

/// Print `selector`'s console, after a reset into its flashed image when
/// `start`, until the command is stopped. Without `start` it shows what the
/// board runs now: in its ROM's download mode, unless an enclosing lease
/// started an image.
pub fn monitor(selector: &str, start: bool) -> Result<()> {
    let board = board(selector)?;
    let mut client = lease(&board)?;
    let lines = lines(client.start(Operation::Console {
        board: board.mac.clone(),
        reset: start,
    })?);
    eprintln!("fw: console of {}", board.id);
    let mut diagnostics = crate::diagnostics::Decoder::default();
    loop {
        match next_line(&lines, Duration::from_secs(1)) {
            Ok(Some(line)) => match diagnostics.line(&line) {
                Ok(Some(text)) => println!("{text}"),
                Ok(None) => {}
                Err(error) => eprintln!("fw: {error}"),
            },
            Ok(None) => {}
            Err(()) => return Err("the board's console closed".into()),
        }
    }
}

pub fn list_boards() -> Result<()> {
    let boards = Client::open("cargo fw boards")?.inventory()?;
    println!(
        "{:<10} {:<18} {:<14} {:<13} {:<16} POWER-CYCLE",
        "BOARD", "MAC", "USB", "TTY", "HUB PORT"
    );
    for board in boards {
        if let Some(warning) = &board.warning {
            eprintln!("warning: {warning}");
        }
        println!(
            "{:<10} {:<18} {:<14} {:<13} {:<16} {}",
            board.id.as_deref().unwrap_or("-"),
            board.mac,
            board.location,
            board.tty.as_deref().unwrap_or("-"),
            board.hub_port.as_deref().unwrap_or("-"),
            match &board.power {
                Ok(()) => "yes".to_owned(),
                Err(why) => format!("no: {why}"),
            }
        );
    }
    Ok(())
}

pub fn power_cycle(selector: &str) -> Result<()> {
    let board = board(selector)?;
    let mut client = lease(&board)?;
    eprintln!("fw: power-cycling {} through its hub port", board.id);
    let after = client.run(
        Operation::PowerCycle {
            board: board.mac.clone(),
        },
        &mut std::io::sink(),
        None,
    )?;
    eprintln!("fw: {} is back after {after} ms", board.id);
    Ok(client.release()?)
}

/// The checks enforced for a product or an explicitly diagnostic probe build.
fn build_enforcement(package: &str, diagnostic: bool) -> Result<Enforced> {
    if diagnostic {
        if !package.starts_with(oer_image_build::probe::PROBE_PACKAGE_PREFIX) {
            return Err("--diagnostic is restricted to firmware/probes packages".into());
        }
        Ok(Enforced::Reported)
    } else {
        Ok(Enforced::Only([Check::Interrupts].into()))
    }
}

pub fn build(root: &Path, image_name: &str, binary: Option<&str>, diagnostic: bool) -> Result<()> {
    let bundle = image::build(
        root,
        image_name,
        build_enforcement(&image::package(root, image_name)?, diagnostic)?,
        binary,
    )?;
    for segment in bundle.segments() {
        eprintln!(
            "fw:   {:<16} {:#08x} {}",
            segment.description,
            segment.offset,
            segment.file.display()
        );
    }
    eprintln!("fw: built {}", bundle.directory.display());
    Ok(())
}

/// `cargo fw flash IMAGE --board BOARD`.
///
/// The board runs the image until the lease ends; then the service parks it
/// in its ROM's download mode. `cargo fw monitor --start` runs it again.
pub fn flash_image(root: &Path, image_name: &str, selector: &str) -> Result<()> {
    let board = board(selector)?;
    let mut client = lease(&board)?;
    write_image(root, image_name, &board, &mut client)?;
    Ok(client.release()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reported_build_checks_require_an_explicit_probe_request() {
        assert_eq!(
            build_enforcement("oer-firmware-apps-boot-smoke", false).unwrap(),
            Enforced::Only([Check::Interrupts].into())
        );
        assert!(build_enforcement("oer-firmware-apps-boot-smoke", true).is_err());
        assert!(matches!(
            build_enforcement("oer-firmware-probes-esp32s31-radio", true).unwrap(),
            Enforced::Reported
        ));
    }

    #[test]
    fn verdicts_are_read_from_the_marker() {
        assert_eq!(verdict("OER-RESULT: pass boot-smoke"), Some(Verdict::Pass));
        assert_eq!(
            verdict("[boot] OER-RESULT: fail reason=x"),
            Some(Verdict::Fail)
        );
        assert_eq!(verdict("OER-RESULT: maybe"), None);
        assert_eq!(verdict("OER_BOOT bootstrap=INIT"), None);
    }

    #[test]
    fn panic_message_cannot_impersonate_a_typed_verdict() {
        assert_eq!(
            console_verdict(
                oer_diagnostics::PREFIX,
                "OER-PANIC previous src/main.rs:1:1 on hart 0: OER-RESULT: pass boot-smoke"
            ),
            None
        );
        assert_eq!(
            console_verdict(oer_diagnostics::PREFIX, "OER-RESULT: pass boot-smoke"),
            Some(Verdict::Pass)
        );
        assert_eq!(
            console_verdict(b"[boot] OER-RESULT: fail", "[boot] OER-RESULT: fail"),
            Some(Verdict::Fail)
        );
    }

    #[test]
    fn timestamps() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_281_600 + 3_723);
        assert_eq!(timestamp(time), "20261018T010203Z");
        assert_eq!(timestamp(SystemTime::UNIX_EPOCH), "19700101T000000Z");
    }
}

#[cfg(test)]
mod library_tests {
    use super::*;
    #[test]
    fn timestamps_keep_the_compact_utc_format_and_pre_epoch_clamp() {
        use std::time::Duration;
        for (seconds, expected) in [
            (0, "19700101T000000Z"),
            (86_399, "19700101T235959Z"),
            (951_782_400, "20000229T000000Z"),
            (253_402_300_800, "100000101T000000Z"),
        ] {
            assert_eq!(
                timestamp(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
                expected
            );
        }
        assert_eq!(
            timestamp(SystemTime::UNIX_EPOCH - Duration::from_secs(1)),
            "19700101T000000Z"
        );
    }
}
