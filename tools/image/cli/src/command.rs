//! The commands of `cargo fw`, called by its thin binary (`tools/image/cli`).

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant, SystemTime},
};

use oer_lab_model as stand;
use oer_lab_router::{router, ssh};

use oer_lab_board::lock::{DeviceAccess, DeviceId};
use oer_lab_board::stand::{LeasedBoard, Via};

use crate::{image, w1, w2};
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

/// The attached stand board `board` of `stand`, under this command's device
/// access: every operation on it goes through the stand's one board path
/// (`oer_lab_board::stand`), as HIL's do.
fn leased(root: &Path, stand: &stand::Stand, board: &stand::Board) -> Result<LeasedBoard> {
    let access = hold(&board.mac)?;
    let attached = oer_lab_board::stand::Board::attached(root, stand, &board.id)?;
    Ok(attached.lease(&access)?)
}

/// The tty of the attached `board`.
fn tty(board: &stand::Board) -> Result<PathBuf> {
    let attached = oer_lab_router::boards::find(&board.mac)
        .ok_or_else(|| format!("board {} ({}) is not attached", board.id, board.mac))?;
    attached.tty.ok_or_else(|| {
        format!(
            "board {} is attached at USB {} without a tty",
            board.id, attached.location
        )
        .into()
    })
}

/// Hold board `mac` through the device lock every writer of the board takes
/// (HIL leases, the stand's flashes), with this command as its holder; a
/// held board is an error naming its holder.
fn hold(mac: &str) -> Result<DeviceAccess> {
    hold_in(&oer_lab_board::lock::directory()?, mac)
}

fn hold_in(directory: &Path, mac: &str) -> Result<DeviceAccess> {
    let id = DeviceId::parse(mac)?;
    let command = std::env::args().collect::<Vec<_>>().join(" ");
    DeviceAccess::try_acquire_in(directory, &id, &command)?
        .map_err(|busy| format!("board {id} is busy: {busy}").into())
}

/// The directory of the stand router's lock: `$XDG_RUNTIME_DIR/open-esp-radio/routers`.
fn router_lock_directory() -> Result<std::path::PathBuf> {
    oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Runtime, "routers")
}

/// Write `image`'s bundle into `board` through the board's one write
/// operation, which records the board's receipt and starts the image.
fn write_image(root: &Path, image: &str, id: &str, board: &LeasedBoard) -> Result<()> {
    let bundle = image::bundle(root, image)?;
    eprintln!("fw: writing {image} to {id}");
    let by = std::env::args().collect::<Vec<_>>().join(" ");
    let receipt = board.flash(
        &oer_lab_board::image::Store::open()?,
        &bundle,
        image,
        Via::Usb,
        &by,
    )?;
    for segment in &receipt.segments {
        eprintln!(
            "fw:   {:<16} {:#08x} {:>8} bytes",
            segment.description, segment.offset, segment.bytes
        );
    }
    Ok(())
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

/// Image w1's stand: `remote` brings the stand router to the W1 profile and
/// reads its AP; the configuration and the lines that may be shown.
fn w1_stand(
    router: &stand::Router<'_>,
    ssid: &str,
    remote: &mut dyn router::Remote,
) -> Result<(w1::Config, Vec<String>)> {
    let profile = router::Profile::new(
        router.radio,
        router.ap_section,
        router.wireless_interface.expose(),
        router.channel,
        ssid,
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
        ssid: w1::Ssid::new(ssid.as_bytes())?,
        country: applied.country,
    };
    Ok((config, shown))
}

/// The stand router's regulatory country, read only, with the router lock.
pub(super) fn w0_country(
    stand: &stand::Stand,
) -> Result<(oer_lab_router::lock::RouterLock, [u8; 2])> {
    let router = stand.router()?;
    let mut ssh = ssh::Ssh::new(router.ssh_target.expose())?;
    let lock = oer_lab_router::lock::router(&router_lock_directory()?, router.ssh_target.expose())?;
    let country = router::radio_country(&mut ssh, router.radio)?;
    eprintln!("fw: w0 regulatory country {}", w1::country_text(country));
    Ok((lock, country))
}

/// The stand router of `stand`, locked and brought to W1's profile; the
/// lock and image w1's configuration.
fn stand_router(
    stand: &stand::Stand,
    image: &str,
) -> Result<(oer_lab_router::lock::RouterLock, w1::Config)> {
    let router = stand.router()?;
    let mut ssh = ssh::Ssh::new(router.ssh_target.expose())?;
    let lock = oer_lab_router::lock::router(&router_lock_directory()?, router.ssh_target.expose())?;
    // The stand AP's SSID is the run's: no file holds it.
    let ssid = oer_lab_router::credentials::ssid(&format!("oer-{image}"))?;
    let (config, shown) = w1_stand(&router, ssid.expose(), &mut ssh)?;
    for line in shown {
        eprintln!("fw: {}", line.replacen("w1", image, 1));
    }
    Ok((lock, config))
}

pub fn run(root: &Path, image: &str, selector: &str, timeout: Duration) -> Result<ExitCode> {
    #[cfg(target_os = "linux")]
    if image == crate::w3::IMAGE {
        return crate::w3::firmware::run(root, selector, timeout);
    }
    let (stand, board) = board(selector)?;
    // Acquire the laptop/air lease before the router lock to keep HIL's lock order.
    let capture = if image == w2::IMAGE {
        let lab = oer_hil_lab::config::LabConfig::load(
            &stand::path()?,
            image::CHIP,
            &oer_hil_lab::config::BoardChoice {
                dut: Some(board.id.clone()),
                ..Default::default()
            },
        )?;
        Some(oer_hil_lab::air::AirCapture::start(
            &lab,
            stand.router()?.channel,
        )?)
    } else {
        None
    };
    // The router lock is held to the end of the run.
    let (_router, built) = if image == w1::IMAGE {
        // The router is changed only for a board that is there.
        tty(&board)?;
        let (lock, config) = stand_router(&stand, w1::IMAGE)?;
        (Some(lock), Some(image::Stand::W1(config)))
    } else if image == w2::IMAGE {
        tty(&board)?;
        let (lock, config) = stand_router(&stand, w2::IMAGE)?;
        (Some(lock), Some(image::Stand::W2(config)))
    } else if image == "w0" {
        let (lock, country) = w0_country(&stand)?;
        (Some(lock), Some(image::Stand::W0 { country }))
    } else {
        (None, None)
    };
    let w1 = match &built {
        Some(image::Stand::W1(config)) => Some(config.clone()),
        _ => None,
    };
    let w2 = match &built {
        Some(image::Stand::W2(config)) => Some(config.clone()),
        _ => None,
    };
    image::build(root, image, built.as_ref(), false, Enforced::Reported, None)?;
    let leased = leased(root, &stand, &board)?;
    write_image(root, image, &board.id, &leased)?;
    let run = root
        .join("target/fw/runs")
        .join(format!("{}-{image}", timestamp(SystemTime::now())));
    std::fs::create_dir_all(&run)?;
    let log_path = run.join("log.txt");
    let mut log = std::fs::File::create(&log_path)?;
    let mut raw_log = std::fs::File::create(run.join("console.raw.txt"))?;
    let mut diagnostics = crate::diagnostics::Decoder::default();
    let lines = leased.reset_into_console()?.lines();
    eprintln!(
        "fw: {} reset into {image}; reading its console for {}s",
        board.id,
        timeout.as_secs()
    );
    let deadline = Instant::now() + timeout;
    // Image w1's console, for its report block.
    let mut console = Vec::new();
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
                    break Some((verdict, text.into_owned()));
                }
                if w1.is_some() || w2.is_some() {
                    console.push(text.into_owned());
                }
            }
            Ok(None) => break None,
            Err(()) => return Err("the board's console closed".into()),
        }
    };
    drop(lines);
    if let Err(error) = diagnostics.finish() {
        eprintln!("fw: {error}");
    }
    eprintln!("fw: log {}", log_path.display());
    let air = capture
        .map(oer_hil_lab::air::AirCapture::finish)
        .transpose()?;
    Ok(match verdict {
        Some((_, result)) if let Some(config) = &w1 => {
            ExitCode::from(check_w1(root, &console, &result, config)?)
        }
        Some((_, result)) if let Some(config) = &w2 => {
            let target = oer_lab_router::mac::Mac::parse(&board.mac).ok_or("invalid board MAC")?;
            match w2::check(
                &console,
                &result,
                config,
                target,
                air.as_deref().unwrap_or_default(),
            ) {
                Ok(checked) => {
                    for line in checked.summary {
                        eprintln!("fw: w2 {line}");
                    }
                    ExitCode::from(if checked.passed { 0 } else { 1 })
                }
                Err(error) => {
                    eprintln!("fw: w2 report contradiction or malformed evidence: {error}");
                    ExitCode::from(3)
                }
            }
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
fn check_w1(root: &Path, console: &[String], result: &str, config: &w1::Config) -> Result<u8> {
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
    if let Ok(report) = w1::Report::parse(console) {
        let profile = oer_chip_profile::Profile::load(root, "esp32s31")?;
        let elf = std::fs::read(root.join("target/fw/w1/runtime.elf"))?;
        let stacks = oer_image_checks::interrupts::interrupt_stacks(root, &profile, elf)?;
        let boot_hart = profile
            .facts
            .interrupts
            .as_ref()
            .ok_or("w1 has no hart facts")?
            .boot_hart;
        let bound = stacks
            .harts
            .iter()
            .find(|hart| hart.core == boot_hart)
            .and_then(|hart| hart.bytes)
            .ok_or("w1 has no proven boot hart IRQ stack bound")?;
        let used = w1::check_interrupt_stack(&report, stacks.allocated_bytes(boot_hart), bound)
            .map_err(|error| error.to_string())?;
        eprintln!(
            "fw: w1 IRQ stack observed {used} <= proven bound {bound} bytes; allocated {} bytes",
            stacks.allocated_bytes(boot_hart)
        );
    }
    Ok(checked.outcome.exit_code())
}

pub fn monitor(root: &Path, selector: &str) -> Result<()> {
    let (stand, board) = board(selector)?;
    let leased = leased(root, &stand, &board)?;
    let lines = leased.open_console()?.lines();
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
    let path = stand::path()?;
    let stand = stand::Stand::load(&path)
        .map_err(|error| eprintln!("warning: {error}"))
        .ok();
    let report = oer_lab_router::hub::status()
        .map_err(|error| eprintln!("warning: {error}"))
        .unwrap_or_default();
    let devices = std::path::Path::new(oer_lab_router::hub::SYSFS_USB_DEVICES);
    println!(
        "{:<10} {:<18} {:<14} {:<13} {:<16} POWER-CYCLE",
        "BOARD", "MAC", "USB", "TTY", "HUB PORT"
    );
    for attached in oer_lab_router::boards::attached() {
        let board = stand
            .as_ref()
            .and_then(|stand| stand.board_by_mac(&attached.mac));
        let id = board.map_or("-", |board| board.id.as_str());
        let detected = oer_lab_router::hub::parent(&attached.location)
            .map_or_else(|| "-".to_owned(), |(hub, port)| format!("{hub}:{port}"));
        let power = match oer_lab_router::hub::power_port(
            devices,
            &report,
            stand.as_ref(),
            &attached.location,
        ) {
            Ok(_) => "yes".to_owned(),
            Err(why) => format!("no: {why}"),
        };
        if let Some(warning) = board.and_then(|board| board.ignored_port(&detected)) {
            eprintln!("warning: {warning}");
        }
        let tty = attached
            .tty
            .as_ref()
            .map_or_else(|| "-".to_owned(), |tty| tty.display().to_string());
        println!(
            "{id:<10} {:<18} {:<14} {tty:<13} {detected:<16} {power}",
            attached.mac, attached.location
        );
    }
    Ok(())
}

pub fn power_cycle(root: &Path, selector: &str) -> Result<()> {
    let (stand, board) = board(selector)?;
    let leased = leased(root, &stand, &board)?;
    eprintln!("fw: power-cycling {} through its hub port", board.id);
    let after = leased.power_cycle_observed()?.verdict()?;
    eprintln!("fw: {} is back after {} ms", board.id, after.as_millis());
    Ok(())
}

/// `cargo fw build IMAGE [--analysis]`.
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

pub fn build(
    root: &Path,
    image_name: &str,
    analysis: bool,
    binary: Option<&str>,
    diagnostic: bool,
) -> Result<()> {
    let bundle = image::build(
        root,
        image_name,
        None,
        analysis,
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
pub fn flash_image(root: &Path, image_name: &str, selector: &str) -> Result<()> {
    let (stand, board) = board(selector)?;
    write_image(root, image_name, &board.id, &leased(root, &stand, &board)?)
}

/// `cargo fw router`.
pub fn router_ready() -> Result<()> {
    let stand = stand::Stand::load(&stand::path()?)?;
    let (_lock, config) = stand_router(&stand, w1::IMAGE)?;
    println!(
        "router ready: the stand AP {} on channel {}",
        config.bssid, config.channel
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reported_build_checks_require_an_explicit_probe_request() {
        assert_eq!(
            build_enforcement("oer-firmware-apps-w0", false).unwrap(),
            Enforced::Only([Check::Interrupts].into())
        );
        assert!(build_enforcement("oer-firmware-apps-w0", true).is_err());
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
channel = 6
channel_width = "ht20"
target_address = "192.168.77.1/24"
client_address = "192.168.77.2/24"
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
                    "wireless.radio9=wifi-device\nwireless.radio9.band='2g'\nwireless.radio9.channel='6'\nwireless.radio9.country='DE'\n\
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
            "Made Up Lab Net",
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
            image::runtime_env("w1", Some(&image::Stand::W1(config.clone()))),
            [
                (w1::ENV_CHANNEL, "6"),
                (w1::ENV_BSSID, "02:5E:C0:DE:F6:D0"),
                (w1::ENV_SSID, "4d616465205570204c6162204e6574"),
                (w1::ENV_COUNTRY, "DE"),
            ]
            .map(|(name, value)| (name.to_owned(), Some(value.to_owned())))
        );
        // An AP on another channel than the profile's is refused; so is an
        // unreachable router, without its name.
        let wrong = w1_stand(
            &router,
            "Made Up Lab Net",
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
            "Made Up Lab Net",
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
            router::plan().unwrap().join("\n"),
        ]);
        let shown = shown.join("\n");
        for secret in SECRETS {
            assert!(!shown.contains(secret), "{secret}: {shown}");
        }
        assert!(shown.contains("xx:xx:xx:xx:F6:D0"), "{shown}");
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
