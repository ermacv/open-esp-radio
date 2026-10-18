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
mod w1;

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant, SystemTime},
};

use clap::{CommandFactory as _, Parser, Subcommand};
use oer_lab::{router, ssh, stand};

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

#[derive(Subcommand)]
enum Command {
    /// Build an image of firmware/apps into target/fw/<image>/.
    ///
    /// Image w1 is built unconfigured here; `run w1` builds it for the
    /// stand.
    Build { image: String },
    /// Write a built image into a board's flash and reset it.
    Flash {
        image: String,
        /// The board's stand id or MAC.
        #[arg(long)]
        board: String,
    },
    /// Build and flash an image, then read the board's console until its
    /// `OER-RESULT:` line: exit 0 on pass, 1 on fail, 2 on a timeout and,
    /// for w1, 3 on a missing, malformed or contradicted report. For w1 the
    /// stand router is first brought to the W1 profile over SSH.
    Run {
        image: String,
        #[arg(long)]
        board: String,
        /// How long to wait for the result, such as `30s` or `2m`.
        #[arg(long, default_value = "30s", value_parser = parse_duration)]
        timeout: Duration,
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

/// Image w1's stand: `remote` brings the stand router to the W1 profile and
/// reads its AP; the configuration and the lines that may be shown.
fn w1_stand(
    router: &stand::Router<'_>,
    remote: &mut dyn router::Remote,
) -> Result<(w1::Config, Vec<String>)> {
    let profile = router::Profile::new(
        router.radio,
        router.ap_section,
        router.wireless_interface.expose(),
        router.channel,
        router.ssid.expose(),
    )?;
    let mut shown = vec![format!(
        "w1 brings the stand router to its profile: {}",
        profile.describe()
    )];
    let applied = router::apply(remote, &profile)?;
    shown.push(format!("w1 {}", applied.describe()));
    let config = w1::Config {
        channel: applied.channel,
        bssid: applied.bssid,
        ssid: w1::Ssid::new(router.ssid.expose().as_bytes())?,
    };
    Ok((config, shown))
}

fn run(root: &Path, image: &str, selector: &str, timeout: Duration) -> Result<ExitCode> {
    let (stand, board) = board(selector)?;
    let w1 = if image == w1::IMAGE {
        // The router is changed only for a board that is there.
        tty(&board)?;
        let router = stand.router()?;
        let mut ssh = ssh::Ssh::new(router.ssh_target.expose())?;
        let (config, shown) = w1_stand(&router, &mut ssh)?;
        for line in shown {
            eprintln!("fw: {line}");
        }
        Some(config)
    } else {
        None
    };
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
            Command::Build { image } => {
                let (manifest, _) = image::build(&root, &image, None)?;
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
            } => return run(&root, &image, &board, timeout),
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

    #[test]
    fn no_image_takes_stand_flags() {
        for line in [
            "fw build w1 --expect-bssid 02:00:00:00:F6:D0",
            "fw build w1 --channel 6",
            "fw run w1 --board b1 --channel 6",
            "fw run w1 --board b1 --expect-bssid 02:00:00:00:F6:D0",
        ] {
            assert!(parse(line).is_err(), "{line}");
        }
        assert!(matches!(
            parse("fw build w1").unwrap().command,
            Command::Build { .. }
        ));
        assert!(matches!(
            parse("fw run w1 --board b1 --timeout 90s").unwrap().command,
            Command::Run { .. }
        ));
    }

    const STAND: &str = r#"
schema = 1

[station]
ssid = "Made Up Lab Net"
passphrase = "made-up-passphrase"

[station_fixture]
kind = "open-wrt"
ssh_target = "made-up-router.lab"
wireless_interface = "phy7-ap3"
radio = "radio9"
ap_section = "lab_ap"
channel = 6

[access_point]
ssid = "made-up-ap-ssid"
passphrase = "made-up-ap-passphrase"
"#;

    const SECRETS: [&str; 9] = [
        "Made Up Lab Net",
        "made-up-passphrase",
        "made-up-router",
        "phy7-ap3",
        "made-up-ap",
        "02:5E:C0:DE",
        "02:5e:c0:de",
        "5E:C0",
        "4d61646520",
    ];

    /// A router answering every command as a W1-ready OpenWrt would, its
    /// `iw` channel `channel`; or failing every command.
    struct Router {
        channel: u8,
        fail: bool,
    }

    impl router::Remote for Router {
        fn run(&mut self, script: &str) -> router::Result<String> {
            if self.fail {
                return Err("ssh could not reach the router or was refused (exit 255)".to_owned());
            }
            Ok(
                if script.starts_with("changes=") || script.starts_with("uci set") {
                    String::new()
                } else if script.starts_with("config=") {
                    // What the router's allowlist filter passes.
                    "wireless.radio9=wifi-device\nwireless.radio9.band='2g'\nwireless.radio9.channel='6'\n\
                 wireless.radio9.htmode='HT20'\nwireless.radio9.beacon_int='100'\n\
                 wireless.radio9.disabled='0'\nwireless.lab_ap=wifi-iface\n\
                 wireless.lab_ap.device='radio9'\nwireless.lab_ap.mode='ap'\n\
                 wireless.lab_ap.ssid='Made Up Lab Net'\nwireless.lab_ap.disabled='0'\n\
                 wireless.lab_ap.hidden='0'\n"
                    .to_owned()
                } else if script.starts_with("phy=") {
                    "\t\t\t* 2437.0 MHz [6] (20.0 dBm)\n\t\t\t* 2462.0 MHz [11] (20.0 dBm)\n"
                        .to_owned()
                } else {
                    format!(
                        "\taddr 02:5e:c0:de:f6:d0\n\tssid Made Up Lab Net\n\ttype AP\n\
                     \tchannel {} ({} MHz), width: 20 MHz, center1: {} MHz\n",
                        self.channel,
                        2407 + 5 * u32::from(self.channel),
                        2407 + 5 * u32::from(self.channel)
                    )
                },
            )
        }
        fn pause(&mut self, _: Duration) {}
    }

    #[test]
    fn the_stand_identity_comes_from_the_router_and_nothing_secret_is_shown() {
        let stand = stand::Stand::parse(STAND).unwrap();
        let router = stand.router().unwrap();
        let (config, lines) = w1_stand(
            &router,
            &mut Router {
                channel: 6,
                fail: false,
            },
        )
        .unwrap();
        assert_eq!(config.channel, 6);
        assert_eq!(config.bssid.masked(), "xx:xx:xx:xx:F6:D0");
        // The runtime build gets the whole identity.
        assert_eq!(
            image::runtime_env("w1", Some(&config)),
            [
                (w1::ENV_CHANNEL, Some("6".to_owned())),
                (w1::ENV_BSSID, Some("02:5E:C0:DE:F6:D0".to_owned())),
                (
                    w1::ENV_SSID,
                    Some("4d616465205570204c6162204e6574".to_owned())
                ),
            ]
        );
        // An AP on another channel than the profile's is refused; so is an
        // unreachable router, without its name.
        let wrong = w1_stand(
            &router,
            &mut Router {
                channel: 11,
                fail: false,
            },
        )
        .unwrap_err();
        assert!(
            wrong.to_string().contains("channel 11, the profile's is 6"),
            "{wrong}"
        );
        let unreachable = w1_stand(
            &router,
            &mut Router {
                channel: 6,
                fail: true,
            },
        )
        .unwrap_err();
        let mut shown = lines;
        shown.extend([
            format!("{stand:?}"),
            format!("{router:?}"),
            format!("{config:?}"),
            wrong.to_string(),
            unreachable.to_string(),
            format!("{:?}", ssh::Ssh::new(router.ssh_target.expose()).unwrap()),
            router::plan().join("\n"),
        ]);
        let shown = shown.join("\n");
        for secret in SECRETS {
            assert!(!shown.contains(secret), "{secret}: {shown}");
        }
        assert!(shown.contains("xx:xx:xx:xx:F6:D0"), "{shown}");
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
