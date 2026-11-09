//! TEMPORARY experiment host program (branch `probe/rx-descriptor-rebind`,
//! never merged). It builds nothing: the probe's bundle is prepared first
//! (`cargo fw build esp32s31-rx-rebind`), outside any lease.
//!
//! Under one lease on the board, the router and the air it writes the
//! bundle, has the router serve its WPA2 network on channel 13 at HT20,
//! joins the laptop's station (`wlan0`, the linux-net fixture) with the
//! explicit legacy profile (`non-ht`: HT, VHT and HE disabled, OFDM rates
//! only), sends UDP from that station over the air to the network's gateway
//! and reads the probe's console from its reset to its `OER-RESULT:` line.
//! The station's transmit bitrate is sampled every second; the router's view
//! of the station's last frame rate is read at the end.
//!
//! The generated SSID and passphrase stay in memory: they reach the router
//! through the LAB service and the station helper through its stdin only.
//! Every exit stops the load first, then the station, then restores the
//! laptop's managed interface state, then releases the lease.
#![forbid(unsafe_code)]

mod console;
mod load;
mod prepared;
mod result;

use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::Path,
    process::{Child, Command, ExitCode, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use oer_tools_lab_arbiter::{
    claims::{AIR, Claim},
    journal::ImageIdentity,
    readiness::ROUTER,
};
use oer_tools_lab_client::{
    client::Client,
    protocol::{FlashVia, LeaseRequest, Operation, RouterChannelWidth, RouterOperation},
};
use serde_json::{Value, json};

use crate::{console::ConsoleSession, load::Load, prepared::Prepared, result::ProbeResult};

const IMAGE: &str = "esp32s31-rx-rebind";
const HELPER: &str = "/usr/local/sbin/open-radio-net";
const WPA_CONTROL: &str = "/run/open-radio-wpa-control";
const CHANNEL: u8 = 13;
const FREQUENCY_MHZ: u32 = 2472;
const DATAGRAM: usize = 1472;
/// The discard port (RFC 863) on the gateway; nothing need answer.
const SINK_PORT: u16 = 9;
/// One blocked UDP send returns within this.
const SEND_TIMEOUT: Duration = Duration::from_millis(100);
/// One `iw` sample returns within this, or is killed.
const SAMPLE_TIMEOUT: Duration = Duration::from_secs(2);
const SAMPLE_PERIOD: Duration = Duration::from_secs(1);
/// From the probe's reset to its result line: boot, RF start, four
/// 2.5-s segments, the final checks and the dump.
const CONSOLE_TIMEOUT: Duration = Duration::from_secs(90);

type Result<T> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["prepare", bundle] => prepare(Path::new(bundle)).map(|()| true),
        ["run", board, bundle, console] => run(board, Path::new(bundle), Path::new(console)),
        _ => {
            eprintln!(
                "usage: oer-hil-scenarios-rx-rebind-load prepare <bundle-directory>\n       \
                 oer-hil-scenarios-rx-rebind-load run <board> <bundle-directory> <console-log>"
            );
            return ExitCode::from(2);
        }
    };
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            eprintln!("load: {error}");
            ExitCode::from(2)
        }
    }
}

fn git(arguments: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(arguments)
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        return Err(format!("git {} failed", arguments.join(" ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Build the image once, outside any lease, and record what was built.
fn prepare(bundle: &Path) -> Result<()> {
    let commit = git(&["rev-parse", "HEAD"])?;
    let dirty = !git(&["status", "--porcelain"])?.is_empty();
    let status = Command::new("cargo")
        .args(["fw", "build", IMAGE])
        .env("CARGO_INCREMENTAL", "0")
        .status()
        .map_err(|error| format!("cargo fw build: {error}"))?;
    if !status.success() {
        return Err(format!("cargo fw build {IMAGE} exited with {status}"));
    }
    let prepared = prepared::write(bundle, &commit, dirty)?;
    eprintln!(
        "load: prepared {IMAGE} from {} (dirty {}), application {}",
        prepared.commit, prepared.dirty, prepared.application_sha256
    );
    Ok(())
}

/// `Ok(true)` when the probe printed its pass line.
fn run(board_id: &str, bundle: &Path, console: &Path) -> Result<bool> {
    let prepared = prepared::read(bundle)?;
    let stand = oer_tools_lab_model::path()
        .and_then(|path| oer_tools_lab_model::Stand::load(&path))
        .map_err(|error| format!("the stand file: {error}"))?;
    let board = stand
        .board(board_id)
        .ok_or_else(|| format!("the stand file has no board `{board_id}`"))?;
    if !stand
        .router
        .as_ref()
        .is_some_and(|router| router.channels.contains(&CHANNEL))
    {
        return Err(format!(
            "the stand file's router does not allow channel {CHANNEL}"
        ));
    }
    let owner = oer_tools_lab_arbiter::owner::from_environment()
        .map_err(|error| error.to_string())?
        .to_string();
    let mut client = Client::open("rx-rebind load")?;
    let granted = client.acquire(
        LeaseRequest {
            owner: owner.clone(),
            work: format!("rx-rebind probe on {board_id} with station load"),
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
        &mut |message| eprintln!("load: {message}"),
    )?;
    eprintln!("load: lease {} granted", granted.lease);
    let mut station = Station::default();
    // `leased` ends its console and its load before it returns, on every
    // path: only then is the station stopped and the laptop restored.
    let ran = leased(
        &mut client,
        &mut station,
        &board.mac,
        &owner,
        bundle,
        &prepared,
        console,
    );
    let restored = station.restore();
    let released = client.release();
    eprintln!("load: lease {} released", granted.lease);
    let passed = ran?;
    restored?;
    released?;
    Ok(passed)
}

/// Random lowercase hex from the kernel's generator.
fn secret(bytes: usize) -> Result<String> {
    let mut random = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut random))
        .map_err(|error| format!("/dev/urandom: {error}"))?;
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[derive(Default)]
struct Station {
    started: bool,
    dhcp: Option<Child>,
}

impl Station {
    fn helper(operation: &str, input: Option<&[u8]>) -> Result<String> {
        let mut child = Command::new("sudo")
            .args(["-n", HELPER, operation])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("{operation}: {error}"))?;
        if let (Some(mut stdin), Some(input)) = (child.stdin.take(), input) {
            stdin
                .write_all(input)
                .map_err(|error| format!("{operation}: {error}"))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("{operation}: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "station helper `{operation}` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn wpa(arguments: &[&str]) -> Result<String> {
        let output = Command::new("wpa_cli")
            .args(["-p", WPA_CONTROL, "-i", "wlan0"])
            .args(arguments)
            .output()
            .map_err(|error| format!("wpa_cli: {error}"))?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Join the router's network with the legacy profile and take a DHCP
    /// lease: the station's address and the gateway.
    fn join(&mut self, ssid: &str, passphrase: &str) -> Result<(Ipv4Addr, Ipv4Addr)> {
        let ssid_hex: String = ssid.bytes().map(|byte| format!("{byte:02x}")).collect();
        let profile = json!({
            "ssid_hex": ssid_hex,
            "security": "wpa2",
            "passphrase": passphrase,
            "frequency_mhz": FREQUENCY_MHZ,
            "phy": "non-ht",
            "ipv4": {"mode": "dhcp"},
        })
        .to_string();
        self.started = true;
        Self::helper("client", Some(profile.as_bytes()))?;
        let enabled = Self::wpa(&["enable_network", "0"])?;
        if enabled.trim() != "OK" {
            return Err(format!("enable_network answered `{}`", enabled.trim()));
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let status = Self::wpa(&["status"])?;
            let field = |name: &str| {
                status
                    .lines()
                    .find_map(|line| line.strip_prefix(name))
                    .map(str::trim)
                    .map(str::to_owned)
            };
            if field("wpa_state=").as_deref() == Some("COMPLETED") {
                eprintln!(
                    "load: station associated, freq={}",
                    field("freq=").unwrap_or_default()
                );
                break;
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "the station did not associate: wpa_state={}",
                    field("wpa_state=").unwrap_or_default()
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let mut dhcp = Command::new("sudo")
            .args(["-n", HELPER, "client-dhcp"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("client-dhcp: {error}"))?;
        let stdout = dhcp.stdout.take().ok_or("client-dhcp has no stdout")?;
        self.dhcp = Some(dhcp);
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(35);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = receiver
                .recv_timeout(left)
                .map_err(|_| "no DHCP lease within 35 s".to_owned())?;
            let event: Value = serde_json::from_str(&line)
                .map_err(|error| format!("client-dhcp line: {error}"))?;
            if event.get("event").and_then(Value::as_str) == Some("bound") {
                let address = |name: &str| {
                    event
                        .pointer(&format!("/lease/{name}"))
                        .and_then(Value::as_str)
                        .and_then(|text| text.parse::<Ipv4Addr>().ok())
                        .ok_or_else(|| format!("the lease has no {name}"))
                };
                return Ok((address("address")?, address("router")?));
            }
        }
    }

    /// Stop the station and restore the laptop's managed interface state.
    /// The legacy profile lives in the station's private supplicant
    /// configuration, which `stop` ends; no other rate setting is made.
    fn restore(&mut self) -> Result<()> {
        if let Some(mut dhcp) = self.dhcp.take() {
            if let Some(mut stdin) = dhcp.stdin.take() {
                let _ = stdin.write_all(b"disconnect\n");
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while dhcp.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = dhcp.kill();
            let _ = dhcp.wait();
        }
        if !self.started {
            return Ok(());
        }
        let stopped = Self::helper("stop", None);
        let managed = Self::helper("managed", None);
        eprintln!(
            "load: station stopped ({}), managed state restored ({})",
            if stopped.is_ok() { "ok" } else { "failed" },
            if managed.is_ok() { "ok" } else { "failed" }
        );
        stopped?;
        managed?;
        Ok(())
    }
}

/// Whether the kernel routes `to` from `from` over the station.
fn routed_over_station(from: Ipv4Addr, to: Ipv4Addr) -> Result<()> {
    let output = Command::new("ip")
        .args([
            "-4",
            "route",
            "get",
            &to.to_string(),
            "from",
            &from.to_string(),
        ])
        .output()
        .map_err(|error| format!("ip route get: {error}"))?;
    let line = String::from_utf8_lossy(&output.stdout);
    let device = line
        .split_whitespace()
        .skip_while(|word| *word != "dev")
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    if device == "wlan0" {
        Ok(())
    } else {
        Err(format!(
            "the gateway is routed over `{device}`, not the station"
        ))
    }
}

/// The station's current transmit bitrate as `iw` reports it, within
/// `SAMPLE_TIMEOUT`: a slower `iw` is killed and the sample says so.
fn tx_bitrate() -> String {
    let Ok(mut child) = Command::new("iw")
        .args(["dev", "wlan0", "link"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return "iw-unavailable".to_owned();
    };
    let deadline = Instant::now() + SAMPLE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return "iw-timeout".to_owned();
            }
        }
    }
    let mut text = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut text);
    }
    text.lines()
        .find_map(|line| line.trim().strip_prefix("tx bitrate:").map(str::trim))
        .map_or_else(|| "no-tx-bitrate".to_owned(), str::to_owned)
}

fn leased(
    client: &mut Client,
    station: &mut Station,
    mac: &str,
    owner: &str,
    bundle: &Path,
    prepared: &Prepared,
    console: &Path,
) -> Result<bool> {
    client.run(
        Operation::Flash {
            board: mac.to_owned(),
            bundle: bundle.display().to_string(),
            image: ImageIdentity {
                name: IMAGE.to_owned(),
                sha256: prepared.application_sha256.clone(),
                commit: Some(prepared.commit.clone()),
                dirty: Some(prepared.dirty),
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
        "load: wrote the probe bundle, application {}",
        prepared.application_sha256
    );
    let ssid = format!("oer-rebind-{}", secret(6)?);
    let passphrase = secret(16)?;
    client.run(
        Operation::Router {
            request: RouterOperation::ApplyStationProfile {
                ssid: ssid.clone(),
                passphrase: passphrase.clone(),
                channel: CHANNEL,
                width: RouterChannelWidth::Ht20,
                forward_host: None,
            },
        },
        &mut std::io::sink(),
        None,
    )?;
    eprintln!("load: router network on channel {CHANNEL}, HT20");
    let (address, gateway) = station.join(&ssid, &passphrase)?;
    drop((ssid, passphrase));
    routed_over_station(address, gateway)?;
    let station_mac = std::fs::read_to_string("/sys/class/net/wlan0/address")
        .map(|text| text.trim().to_owned())
        .map_err(|error| format!("wlan0 address: {error}"))?;
    eprintln!(
        "load: station lease taken, legacy profile; tx bitrate {}",
        tx_bitrate()
    );
    let socket = UdpSocket::bind(SocketAddrV4::new(address, 0))
        .and_then(|socket| {
            socket
                .connect(SocketAddrV4::new(gateway, SINK_PORT))
                .map(|()| socket)
        })
        .and_then(|socket| {
            socket
                .set_write_timeout(Some(SEND_TIMEOUT))
                .map(|()| socket)
        })
        .map_err(|error| format!("load socket: {error}"))?;
    let payload = [0x5a_u8; DATAGRAM];
    // Declared before the console: on an early return the console ends
    // first, then the load, both before the station is restored.
    let load = Load::start(
        move || socket.send(&payload).map(|_| ()),
        tx_bitrate,
        SAMPLE_PERIOD,
    );
    std::thread::sleep(Duration::from_secs(2));
    eprintln!("load: UDP running; reading the probe from its reset");
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
    // The router's view of the station's last frames, while it still sends.
    let link = client.run(
        Operation::Router {
            request: RouterOperation::StationLink { mac: station_mac },
        },
        &mut std::io::sink(),
        None,
    );
    let report = load.finish();
    eprintln!(
        "load: sent {} datagrams of {DATAGRAM} bytes in {:.1} s ({:.1} Mbit/s), {} refused or timed-out sends",
        report.datagrams,
        report.seconds,
        report.datagrams as f64 * DATAGRAM as f64 * 8.0 / report.seconds / 1e6,
        report.refused
    );
    eprintln!(
        "load: station tx bitrate samples: {}",
        report.samples.join(" | ")
    );
    match &link {
        Ok(link) => eprintln!(
            "load: router: station rx_bitrate {} tx_bitrate {} rx_packets {}",
            link.get("rx_bitrate").unwrap_or(&Value::Null),
            link.get("tx_bitrate").unwrap_or(&Value::Null),
            link.get("rx_packets").unwrap_or(&Value::Null)
        ),
        Err(error) => eprintln!("load: router station link unavailable: {error}"),
    }
    match verdict? {
        ProbeResult::Pass => Ok(true),
        ProbeResult::Fail(reason) => {
            eprintln!("load: probe result: fail {reason}");
            Ok(false)
        }
    }
}
