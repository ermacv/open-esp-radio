//! `cargo fw`: build, flash and run the staged images of `firmware/apps` on
//! the stand's boards (README.md).

mod boards;
mod chip;
mod encode;
mod flash;
mod image;
mod lock;
mod pack;
mod placement;
mod port;
mod power;
mod stand;
mod w1;

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant, SystemTime},
};

use clap::{Args, CommandFactory as _, Parser, Subcommand};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The chips whose boards this tool resets and writes. Another chip's board
/// is only listed: its reset lines may mean something else (an ESP32-C5's
/// RTS reset enters its download mode).
const SUPPORTED_CHIPS: [&str; 1] = [image::CHIP];

/// The marker an image prints with its verdict.
const RESULT_MARKER: &str = "OER-RESULT: ";

#[derive(Parser)]
#[command(about = "Build, flash and run staged firmware images on the stand's boards")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Image w1's parameters, which no other image takes.
#[derive(Args, Clone, Copy)]
struct W1Args {
    /// Image w1: the stand AP's BSSID it must hear, or OER_W1_EXPECT_BSSID.
    /// Only its last two octets are ever shown.
    #[arg(long, value_name = "MAC", value_parser = BssidParser)]
    expect_bssid: Option<w1::Bssid>,
    /// Image w1: the 2.4 GHz channel 1 to 13 it listens on, or
    /// OER_W1_CHANNEL.
    #[arg(long, value_parser = w1::parse_channel)]
    channel: Option<u8>,
}

impl W1Args {
    /// The parameters of `image` from these flags or the environment.
    fn params(self, image: &str, required: bool) -> Result<Option<w1::Config>> {
        let env =
            |name: &str| std::env::var_os(name).map(|value| value.to_string_lossy().into_owned());
        Ok(w1::params(
            image,
            self.expect_bssid,
            self.channel,
            &env,
            required,
        )?)
    }
}

/// [`w1::parse_bssid`] for clap, whose own error would repeat the value.
#[derive(Clone, Copy)]
struct BssidParser;

impl clap::builder::TypedValueParser for BssidParser {
    type Value = w1::Bssid;

    fn parse_ref(
        &self,
        command: &clap::Command,
        argument: Option<&clap::Arg>,
        value: &std::ffi::OsStr,
    ) -> std::result::Result<w1::Bssid, clap::Error> {
        let name = argument.map_or_else(|| "--expect-bssid".to_owned(), ToString::to_string);
        value
            .to_str()
            .ok_or_else(|| "the BSSID is no text".to_owned())
            .and_then(w1::parse_bssid)
            .map_err(|reason| {
                clap::Error::raw(
                    clap::error::ErrorKind::ValueValidation,
                    format!("invalid value for '{name}': {reason}\n"),
                )
                .with_cmd(command)
            })
    }
}

#[derive(Subcommand)]
enum Command {
    /// Build an image of firmware/apps into target/fw/<image>/.
    Build {
        image: String,
        #[command(flatten)]
        w1: W1Args,
    },
    /// Write a built image into a board's flash and reset it.
    Flash {
        image: String,
        /// The board's stand id or MAC.
        #[arg(long)]
        board: String,
    },
    /// Build and flash an image, then read the board's console until its
    /// `OER-RESULT:` line: exit 0 on pass, 1 on fail, 2 on a timeout and,
    /// for w1, 3 on a missing, malformed or contradicted report.
    Run {
        image: String,
        #[arg(long)]
        board: String,
        /// How long to wait for the result, such as `30s` or `2m`.
        #[arg(long, default_value = "30s", value_parser = parse_duration)]
        timeout: Duration,
        #[command(flatten)]
        w1: W1Args,
    },
    /// Print a board's console without resetting it.
    Monitor {
        #[arg(long)]
        board: String,
    },
    /// List the attached boards and their stand ids.
    Boards,
    /// Switch a board's hub port off and on again.
    PowerCycle {
        #[arg(long)]
        board: String,
    },
}

/// `30s`, `2m` or a number of seconds.
fn parse_duration(text: &str) -> std::result::Result<Duration, String> {
    let (number, unit) = match text.strip_suffix('s') {
        Some(number) => (number, 1),
        None => match text.strip_suffix('m') {
            Some(number) => (number, 60),
            None => (text, 1),
        },
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(unit))
        .map(Duration::from_secs)
        .ok_or_else(|| format!("`{text}` is no duration such as 30s or 2m"))
}

/// The repository this tool runs in.
fn root() -> Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    Ok(std::fs::canonicalize(manifest.join("../.."))?)
}

/// The stand board `selector` names, which this tool may reset and write.
fn board(selector: &str) -> Result<(stand::Stand, stand::Board)> {
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
    Ok((stand, board))
}

/// The tty of the attached `board`.
fn tty(board: &stand::Board) -> Result<PathBuf> {
    let attached = boards::find(&board.mac)
        .ok_or_else(|| format!("board {} ({}) is not attached", board.id, board.mac))?;
    attached.tty.ok_or_else(|| {
        format!(
            "board {} is attached at USB {} without a tty",
            board.id, attached.location
        )
        .into()
    })
}

fn write_image(root: &Path, image: &str, board: &stand::Board, after: flash::After) -> Result<()> {
    let (manifest, segments) = image::segments(root, image)?;
    if manifest.chip != board.chip {
        return Err(format!(
            "image {image} is for {}, board {} is an {}",
            manifest.chip, board.id, board.chip
        )
        .into());
    }
    let tty = tty(board)?;
    eprintln!(
        "fw: writing {image} to {} through {}",
        board.id,
        tty.display()
    );
    for written in flash::write(&tty, &board.mac, &board.chip, &segments, after)? {
        eprintln!(
            "fw:   {:<16} {:#08x} {:>8} bytes {}",
            written.name,
            written.offset,
            written.bytes,
            if written.skipped {
                "unchanged, skipped"
            } else {
                "written"
            }
        );
    }
    Ok(())
}

/// Retry opening `tty` for a moment: the flasher's port is released, or the
/// board returns, a moment later.
fn reopen(tty: &Path) -> Result<serialport::TTYPort> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match port::open_kept(tty, Duration::from_millis(100)) {
            Ok(port) => return Ok(port),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(error),
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

/// `YYYYMMDDTHHMMSSZ` of `time`.
fn timestamp(time: SystemTime) -> String {
    let seconds = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let of_day = seconds % 86_400;
    // Howard Hinnant's `civil_from_days`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60
    )
}

fn run(
    root: &Path,
    image: &str,
    selector: &str,
    timeout: Duration,
    w1: Option<w1::Config>,
) -> Result<ExitCode> {
    let (_stand, board) = board(selector)?;
    if let Some(config) = &w1 {
        eprintln!(
            "fw: {image} expects {} on channel {}",
            config.expect.masked(),
            config.channel
        );
    }
    image::build(root, image, w1.as_ref())?;
    let _lock = lock::acquire(&board.mac)?;
    write_image(root, image, &board, flash::After::StayInStub)?;
    let run = root
        .join("target/fw/runs")
        .join(format!("{}-{image}", timestamp(SystemTime::now())));
    std::fs::create_dir_all(&run)?;
    let log_path = run.join("log.txt");
    let mut log = std::fs::File::create(&log_path)?;
    let tty = tty(&board)?;
    let mut serial = reopen(&tty)?;
    port::reset_into_application(&mut serial)?;
    eprintln!(
        "fw: {} reset into {image}; reading its console for {}s",
        board.id,
        timeout.as_secs()
    );
    let lines = port::lines(serial);
    let deadline = Instant::now() + timeout;
    // Image w1's console, for its report block.
    let mut console = Vec::new();
    let verdict = loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break None;
        };
        match lines.next(remaining) {
            Ok(Some(line)) => {
                let text = String::from_utf8_lossy(&line);
                println!("{text}");
                writeln!(log, "{text}")?;
                if let Some(verdict) = verdict(&text) {
                    break Some((verdict, text.into_owned()));
                }
                if w1.is_some() {
                    console.push(text.into_owned());
                }
            }
            Ok(None) => break None,
            Err(()) => return Err(format!("the console {} closed", tty.display()).into()),
        }
    };
    drop(lines);
    eprintln!("fw: log {}", log_path.display());
    Ok(match verdict {
        Some((_, result)) if let Some(config) = &w1 => {
            ExitCode::from(check_w1(&console, &result, config))
        }
        Some((Verdict::Pass, _)) => ExitCode::SUCCESS,
        Some((Verdict::Fail, _)) => ExitCode::from(1),
        None => {
            eprintln!("fw: no {RESULT_MARKER}line within {}s", timeout.as_secs());
            ExitCode::from(2)
        }
    })
}

/// Check image w1's report against its `result` line and print the host's
/// summary; the exit code.
fn check_w1(console: &[String], result: &str, config: &w1::Config) -> u8 {
    let checked = w1::check(console, result, config);
    for line in &checked.summary {
        eprintln!("fw: w1 {line}");
    }
    match &checked.outcome {
        w1::Outcome::Pass => eprintln!("fw: w1 pass: the host agrees with every gate"),
        w1::Outcome::Fail(reasons) => {
            eprintln!("fw: w1 fail, the host agrees: {}", reasons.join("; "));
        }
        w1::Outcome::Contradiction(problems) => {
            for problem in problems {
                eprintln!("fw: w1 contradiction: {problem}");
            }
        }
        w1::Outcome::Unreadable(error) => eprintln!("fw: w1 {error}"),
    }
    checked.outcome.exit_code()
}

fn monitor(selector: &str) -> Result<()> {
    let (_stand, board) = board(selector)?;
    let _lock = lock::acquire(&board.mac)?;
    let tty = tty(&board)?;
    let lines = port::lines(port::open_without_reset(&tty)?);
    eprintln!("fw: console of {} at {}", board.id, tty.display());
    loop {
        match lines.next(Duration::from_secs(1)) {
            Ok(Some(line)) => println!("{}", String::from_utf8_lossy(&line)),
            Ok(None) => {}
            Err(()) => return Err(format!("the console {} closed", tty.display()).into()),
        }
    }
}

fn list_boards() -> Result<()> {
    let path = stand::path()?;
    let stand = stand::Stand::load(&path)
        .map_err(|error| eprintln!("warning: {error}"))
        .ok();
    println!(
        "{:<10} {:<18} {:<14} {:<13} STAND PORT",
        "BOARD", "MAC", "USB", "TTY"
    );
    for attached in boards::attached() {
        let known = stand
            .as_ref()
            .and_then(|stand| Some((stand, stand.board_by_mac(&attached.mac)?)));
        let id = known.map_or("-", |(_, board)| board.id.as_str());
        let port = known.map_or_else(
            || "-".to_owned(),
            |(stand, board)| match &board.port {
                None => "-".to_owned(),
                Some(port) => {
                    let location = stand
                        .hub(&port.hub)
                        .map(|hub| format!("{}.{}", hub.usb2, port.port));
                    let agrees = location.as_deref() == Some(attached.location.as_str());
                    format!(
                        "{}:{}{}",
                        port.hub,
                        port.port,
                        if agrees { "" } else { " (disagrees with USB)" }
                    )
                }
            },
        );
        let tty = attached
            .tty
            .as_ref()
            .map_or_else(|| "-".to_owned(), |tty| tty.display().to_string());
        println!(
            "{id:<10} {:<18} {:<14} {tty:<13} {port}",
            attached.mac, attached.location
        );
    }
    Ok(())
}

fn power_cycle(selector: &str) -> Result<()> {
    let (stand, board) = board(selector)?;
    let _lock = lock::acquire(&board.mac)?;
    let attached = boards::find(&board.mac);
    let (hub, port) = power::port_of(&stand, &board, attached.as_ref())?;
    eprintln!("fw: power-cycling {} on {} port {port}", board.id, hub.id);
    let returned = power::cycle(hub, port, &board.mac)?;
    eprintln!(
        "fw: {} is back at USB {} ({})",
        board.id,
        returned.location,
        returned
            .tty
            .as_ref()
            .map_or_else(String::new, |tty| tty.display().to_string())
    );
    Ok(())
}

fn main() -> ExitCode {
    if oer_command_tree::requested() {
        let tree = oer_command_tree::command_tree(&Cli::command(), &[String::from("fw")]);
        println!("{}", oer_command_tree::json(&tree));
        return ExitCode::SUCCESS;
    }
    let cli = Cli::parse();
    let result = (|| -> Result<ExitCode> {
        let root = root()?;
        match cli.command {
            Command::Build { image, w1 } => {
                let params = w1.params(&image, false)?;
                if let Some(config) = &params {
                    eprintln!(
                        "fw: {image} expects {} on channel {}",
                        config.expect.masked(),
                        config.channel
                    );
                }
                let (manifest, _) = image::build(&root, &image, params.as_ref())?;
                for segment in &manifest.segments {
                    eprintln!(
                        "fw:   {:<16} {:#08x} {}",
                        segment.name, segment.offset, segment.file
                    );
                }
                eprintln!("fw: built {}", image::output(&root, &image).display());
            }
            Command::Flash {
                image,
                board: selector,
            } => {
                let (_stand, board) = board(&selector)?;
                let _lock = lock::acquire(&board.mac)?;
                write_image(&root, &image, &board, flash::After::HardReset)?;
            }
            Command::Run {
                image,
                board,
                timeout,
                w1,
            } => {
                let params = w1.params(&image, true)?;
                return run(&root, &image, &board, timeout, params);
            }
            Command::Monitor { board } => monitor(&board)?,
            Command::Boards => list_boards()?,
            Command::PowerCycle { board } => power_cycle(&board)?,
        }
        Ok(ExitCode::SUCCESS)
    })();
    result.unwrap_or_else(|error| {
        eprintln!("fw: {error}");
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn parse(line: &str) -> std::result::Result<Cli, clap::Error> {
        Cli::try_parse_from(line.split(' '))
    }

    fn run_params(line: &str) -> Result<Option<w1::Config>> {
        match parse(line)?.command {
            Command::Run { image, w1, .. } => w1.params(&image, true),
            Command::Build { image, w1 } => w1.params(&image, false),
            _ => Err("no build or run".into()),
        }
    }

    #[test]
    fn w1_flags_are_validated() {
        let config =
            run_params("fw run w1 --board b1 --expect-bssid 02:00:00:00:f6:d0 --channel 6")
                .unwrap()
                .unwrap();
        assert_eq!(config.channel, 6);
        assert_eq!(config.expect.masked(), "xx:xx:xx:xx:F6:D0");
        for channel in ["0", "14"] {
            let line = format!(
                "fw run w1 --board b1 --expect-bssid 02:00:00:00:F6:D0 --channel {channel}"
            );
            assert!(parse(&line).is_err(), "{channel}");
        }
        // clap refuses a bad BSSID without repeating it.
        for bssid in ["03:AB:CD:EF:F6:D0", "02:AB:CD:EF:F6", "00:00:00:00:00:00"] {
            let error = parse(&format!("fw build w1 --expect-bssid {bssid} --channel 6"))
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains("--expect-bssid"), "{error}");
            assert!(
                !error.contains("AB:CD") && !error.contains(bssid),
                "{error}"
            );
        }
        // Another image refuses them; w1's run needs both, its build both or
        // neither.
        assert!(run_params("fw run w0 --board b1 --channel 6").is_err());
        assert!(run_params("fw build boot-smoke --expect-bssid 02:00:00:00:F6:D0").is_err());
        assert!(run_params("fw run w0 --board b1").unwrap().is_none());
        if std::env::var_os(w1::ENV_BSSID).is_none() && std::env::var_os(w1::ENV_CHANNEL).is_none()
        {
            assert!(run_params("fw run w1 --board b1 --channel 6").is_err());
            assert!(run_params("fw build w1").unwrap().is_none());
        }
    }

    #[test]
    fn durations_and_timestamps() {
        assert_eq!(parse_duration("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_secs(120)));
        assert_eq!(parse_duration("5"), Ok(Duration::from_secs(5)));
        assert!(parse_duration("soon").is_err());
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_281_600 + 3_723);
        assert_eq!(timestamp(time), "20261018T010203Z");
        assert_eq!(timestamp(SystemTime::UNIX_EPOCH), "19700101T000000Z");
    }
}
