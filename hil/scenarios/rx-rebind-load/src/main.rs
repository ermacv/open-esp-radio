//! TEMPORARY experiment host program (branch `probe/rx-descriptor-rebind`,
//! never merged). One lease on the board, the router and the air: the
//! router's WPA2 network on channel 13 at HT20, the laptop's station
//! (`wlan0`, the linux-net fixture) associated to it, UDP load from that
//! station over the air to the network's gateway while the rx-rebind probe
//! image runs under the same lease through `cargo fw run`. The generated
//! SSID and passphrase stay in memory and reach the router through the LAB
//! service and the station helper through its stdin only. The station is
//! stopped and the laptop's managed state restored on every outcome.
#![forbid(unsafe_code)]

use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    process::{Child, Command, ExitCode, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use oer_tools_lab_arbiter::{
    claims::{AIR, Claim},
    readiness::ROUTER,
};
use oer_tools_lab_client::{
    client::Client,
    protocol::{LEASE_ENV, LeaseRequest, Operation, RouterChannelWidth, RouterOperation},
};
use serde_json::{Value, json};

const HELPER: &str = "/usr/local/sbin/open-radio-net";
const WPA_CONTROL: &str = "/run/open-radio-wpa-control";
const CHANNEL: u8 = 13;
const FREQUENCY_MHZ: u32 = 2472;
const DATAGRAM: usize = 1472;
/// The discard port (RFC 863) on the gateway; nothing need answer.
const SINK_PORT: u16 = 9;

type Result<T> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let (Some(board), Some(console), None) = (arguments.next(), arguments.next(), arguments.next())
    else {
        eprintln!("usage: oer-hil-scenarios-rx-rebind-load <board> <console-log>");
        return ExitCode::from(2);
    };
    match run(&board, &console) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("load: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(board_id: &str, console: &str) -> Result<u8> {
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
            owner,
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
    let token = client.token().map(str::to_owned).ok_or("no lease token")?;
    let mut station = Station::default();
    let ran = leased(&mut client, &mut station, board_id, console, &token);
    let restored = station.restore();
    let released = client.release();
    eprintln!("load: lease {} released", granted.lease);
    let code = ran?;
    restored?;
    released?;
    Ok(code)
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

    /// Join the router's network and take a DHCP lease: the station's
    /// address and the gateway.
    fn join(&mut self, ssid: &str, passphrase: &str) -> Result<(Ipv4Addr, Ipv4Addr)> {
        let ssid_hex: String = ssid.bytes().map(|byte| format!("{byte:02x}")).collect();
        let profile = json!({
            "ssid_hex": ssid_hex,
            "security": "wpa2",
            "passphrase": passphrase,
            "frequency_mhz": FREQUENCY_MHZ,
            "phy": "ht",
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
        let (sender, receiver) = std::sync::mpsc::channel();
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

fn leased(
    client: &mut Client,
    station: &mut Station,
    board: &str,
    console: &str,
    token: &str,
) -> Result<u8> {
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
    eprintln!("load: station has its lease; UDP to the gateway's discard port");
    let stop = Arc::new(AtomicBool::new(false));
    let sent = Arc::new(AtomicU64::new(0));
    let refused = Arc::new(AtomicU64::new(0));
    let socket = UdpSocket::bind(SocketAddrV4::new(address, 0))
        .and_then(|socket| {
            socket
                .connect(SocketAddrV4::new(gateway, SINK_PORT))
                .map(|()| socket)
        })
        .map_err(|error| format!("load socket: {error}"))?;
    let flood = {
        let (stop, sent, refused) = (stop.clone(), sent.clone(), refused.clone());
        std::thread::spawn(move || {
            let payload = [0x5a_u8; DATAGRAM];
            while !stop.load(Ordering::Relaxed) {
                match socket.send(&payload) {
                    Ok(_) => sent.fetch_add(1, Ordering::Relaxed),
                    Err(_) => refused.fetch_add(1, Ordering::Relaxed),
                };
            }
        })
    };
    let started = Instant::now();
    std::thread::sleep(Duration::from_secs(2));
    let log = std::fs::File::create(console).map_err(|error| format!("{console}: {error}"))?;
    eprintln!("load: running the probe on {board}");
    let status = Command::new("cargo")
        .args([
            "fw",
            "run",
            "esp32s31-rx-rebind",
            "--board",
            board,
            "--timeout",
            "90s",
        ])
        .env(LEASE_ENV, token)
        .stdout(log)
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("cargo fw run: {error}"));
    stop.store(true, Ordering::Relaxed);
    let _ = flood.join();
    let seconds = started.elapsed().as_secs_f64();
    let datagrams = sent.load(Ordering::Relaxed);
    eprintln!(
        "load: sent {datagrams} datagrams of {DATAGRAM} bytes in {seconds:.1} s ({:.1} Mbit/s), {} refused sends",
        datagrams as f64 * DATAGRAM as f64 * 8.0 / seconds / 1e6,
        refused.load(Ordering::Relaxed)
    );
    let status = status?;
    eprintln!("load: cargo fw run exited with {status}");
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(2))
}
