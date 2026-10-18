//! The commands of `cargo fw`, called by its thin binary (`tools/cli/fw`).

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant, SystemTime},
};

use oer_tool_data_stand_file as stand;
use oer_tool_service_lab::{router, ssh};

use oer_tool_service_device_lock::{DeviceAccess, DeviceId};

use crate::{Result, flash, image, port, power, w1};

/// The chips whose boards this tool resets and writes. Another chip's board
/// is only listed: its reset lines may mean something else (an ESP32-C5's
/// RTS reset enters its download mode).
const SUPPORTED_CHIPS: [&str; 1] = [image::CHIP];

/// The marker an image prints with its verdict.
const RESULT_MARKER: &str = "OER-RESULT: ";

/// The repository this tool runs in.
pub fn root() -> Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    Ok(std::fs::canonicalize(manifest.join("../../../.."))?)
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
    let attached = oer_tool_service_lab::boards::find(&board.mac)
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
    hold_in(&oer_tool_service_device_lock::directory()?, mac)
}

fn hold_in(directory: &Path, mac: &str) -> Result<DeviceAccess> {
    let id = DeviceId::parse(mac)?;
    let command = std::env::args().collect::<Vec<_>>().join(" ");
    DeviceAccess::try_acquire_in(directory, &id, &command)?
        .map_err(|busy| format!("board {id} is busy: {busy}").into())
}

/// The directory of the stand router's lock: `$XDG_RUNTIME_DIR/open-esp-radio/routers`.
fn router_lock_directory() -> Result<std::path::PathBuf> {
    oer_tool_base_durable::xdg::path(oer_tool_base_durable::xdg::Base::Runtime, "routers")
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
    };
    Ok((config, shown))
}

/// The stand router of `stand`, locked and brought to W1's profile; the
/// lock and image w1's configuration.
fn w1_router(stand: &stand::Stand) -> Result<(oer_tool_service_lab::lock::RouterLock, w1::Config)> {
    let router = stand.router()?;
    let mut ssh = ssh::Ssh::new(router.ssh_target.expose())?;
    let lock =
        oer_tool_service_lab::lock::router(&router_lock_directory()?, router.ssh_target.expose())?;
    // The stand AP's SSID is the run's: no file holds it.
    let ssid = oer_tool_service_lab::credentials::ssid("oer-w1")?;
    let (config, shown) = w1_stand(&router, ssid.expose(), &mut ssh)?;
    for line in shown {
        eprintln!("fw: {line}");
    }
    Ok((lock, config))
}

pub fn run(root: &Path, image: &str, selector: &str, timeout: Duration) -> Result<ExitCode> {
    let (stand, board) = board(selector)?;
    // The router lock is held to the end of the run.
    let (_router, w1) = if image == w1::IMAGE {
        // The router is changed only for a board that is there.
        tty(&board)?;
        let (lock, config) = w1_router(&stand)?;
        (Some(lock), Some(config))
    } else {
        (None, None)
    };
    image::build(root, image, w1.as_ref())?;
    let _board = hold(&board.mac)?;
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

pub fn monitor(selector: &str) -> Result<()> {
    let (_stand, board) = board(selector)?;
    let _board = hold(&board.mac)?;
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

pub fn list_boards() -> Result<()> {
    let path = stand::path()?;
    let stand = stand::Stand::load(&path)
        .map_err(|error| eprintln!("warning: {error}"))
        .ok();
    let report = oer_tool_service_lab::hub::status()
        .map_err(|error| eprintln!("warning: {error}"))
        .unwrap_or_default();
    let devices = std::path::Path::new(oer_tool_service_lab::hub::SYSFS_USB_DEVICES);
    println!(
        "{:<10} {:<18} {:<14} {:<13} {:<16} POWER-CYCLE",
        "BOARD", "MAC", "USB", "TTY", "HUB PORT"
    );
    for attached in oer_tool_service_lab::boards::attached() {
        let board = stand
            .as_ref()
            .and_then(|stand| stand.board_by_mac(&attached.mac));
        let id = board.map_or("-", |board| board.id.as_str());
        let detected = oer_tool_service_lab::hub::parent(&attached.location)
            .map_or_else(|| "-".to_owned(), |(hub, port)| format!("{hub}:{port}"));
        let power = match oer_tool_service_lab::hub::power_port(
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

pub fn power_cycle(selector: &str) -> Result<()> {
    let (stand, board) = board(selector)?;
    let _board = hold(&board.mac)?;
    let attached = oer_tool_service_lab::boards::find(&board.mac);
    let report = oer_tool_service_lab::hub::status()?;
    let port = power::port_of(
        Some(&stand),
        &board.id,
        attached.as_ref(),
        &report,
        std::path::Path::new(oer_tool_service_lab::hub::SYSFS_USB_DEVICES),
    )?;
    if let Some(warning) = board.ignored_port(&format!("{}:{}", port.hub, port.port)) {
        eprintln!("warning: {warning}");
    }
    eprintln!("fw: power-cycling {} on {port}", board.id);
    let returned = power::cycle(&port, &board.mac)?;
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

/// `cargo fw build IMAGE`.
pub fn build(root: &Path, image_name: &str) -> Result<()> {
    let (manifest, _) = image::build(root, image_name, None)?;
    for segment in &manifest.segments {
        eprintln!(
            "fw:   {:<16} {:#08x} {}",
            segment.name, segment.offset, segment.file
        );
    }
    eprintln!("fw: built {}", image::output(root, image_name).display());
    Ok(())
}

/// `cargo fw flash IMAGE --board BOARD`.
pub fn flash_image(root: &Path, image_name: &str, selector: &str) -> Result<()> {
    let (_stand, board) = board(selector)?;
    let _board = hold(&board.mac)?;
    write_image(root, image_name, &board, flash::After::HardReset)
}

/// `cargo fw router`.
pub fn router_ready() -> Result<()> {
    let stand = stand::Stand::load(&stand::path()?)?;
    let (_lock, config) = w1_router(&stand)?;
    println!(
        "router ready: the stand AP {} on channel {}",
        config.bssid, config.channel
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set in the child that holds a board for
    /// `a_board_another_process_holds_is_refused`: the lock directory.
    const HOLDER_DIRECTORY: &str = "OER_FW_TEST_HOLDER_DIRECTORY";
    const BOARD: &str = "02:00:00:00:01:D0";

    /// In the child only: hold the board through the device lock as a HIL
    /// lease would, say so, and keep it until the parent ends the child.
    #[test]
    fn hold_the_board_for_the_parent() {
        let Some(directory) = std::env::var_os(HOLDER_DIRECTORY) else {
            return;
        };
        let id = DeviceId::parse(BOARD).unwrap();
        let _held = DeviceAccess::try_acquire_in(Path::new(&directory), &id, "hil lease")
            .unwrap()
            .unwrap();
        println!("held");
        std::io::stdout().flush().unwrap();
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn a_board_another_process_holds_is_refused() {
        use std::io::BufRead as _;
        let directory = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "command::tests::hold_the_board_for_the_parent",
                "--nocapture",
            ])
            .env(HOLDER_DIRECTORY, directory.path())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
        assert!(lines.any(|line| line.unwrap() == "held"));
        let error = hold_in(directory.path(), BOARD).unwrap_err().to_string();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(
            error.contains("is busy") && error.contains("hil lease"),
            "{error}"
        );
        // The holder gone, the board is free.
        let mut free = None;
        for _ in 0..100 {
            if let Ok(access) = hold_in(directory.path(), BOARD) {
                free = Some(access);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(free.is_some());
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
            router::plan().join("\n"),
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
