//! The HIL Wi-Fi station load scenario. The image's station joins the lab
//! router's isolated network, which forwards UDP between it and this host
//! (wired to the router: one hop over the air). The host sends and receives
//! the load; the image counts its end and reports its CPU over the HIL link.
//! RX and TX are named from the image's side. Every level of every mode is
//! measured and reported (`target/hil/results/`): the measurements never
//! decide a case, only a path or an exchange that does not work does.
#![forbid(unsafe_code)]

mod traffic;

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    process::ExitCode,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use oer_hil_agent::base::Service as _;
use oer_hil_harness::{Case, Cx, Failure, RouterOperation, Scenario, check};
use oer_services_load::{Collect, Collected, GetCpu, Load, Report, Start, Started, Transmit};
use oer_services_wifi_station::{GetStatus, Join, Joined, Status};

const SCENARIO: Scenario = Scenario::new(
    "wifi-load",
    "esp32s31-wifi-station",
    &[
        oer_services_wifi_station::Request::KEYS,
        oer_services_load::Request::KEYS,
    ],
)
.using_router();

const CASES: [Case; 4] = [
    Case {
        name: "path",
        run: path,
    },
    Case {
        name: "rx",
        run: rx,
    },
    Case {
        name: "tx",
        run: tx,
    },
    Case {
        name: "rx-tx",
        run: rx_tx,
    },
];

/// The station network the router serves (`tools/lab/router/README.md`).
const STATION_NETWORK: &str = "10.231.0.0/24";
const ROUTER_ADDRESS: [u8; 4] = [10, 231, 0, 1];
/// The image's load port.
const DEVICE_PORT: u16 = 5001;
/// UDP payload of a load datagram.
const PAYLOAD: usize = 1400;
/// Total offered load of each level, Mbit/s; a mode with two directions
/// offers half each way. The last is above what HT20 carries.
const LEVELS_MBPS: [u64; 5] = [10, 30, 50, 65, 100];
/// How long each level sends.
const DURATION: Duration = Duration::from_secs(5);
/// How long the image waits after `Start` before it sends.
const DEVICE_DELAY_MS: u32 = 300;
/// What may still be in flight after the senders stop.
const DRAIN: Duration = Duration::from_millis(500);
/// RTT probes during a level.
const PROBE_PERIOD: Duration = Duration::from_millis(50);
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);
const DHCP_TIMEOUT: Duration = Duration::from_secs(30);
const CALL: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    oer_hil_harness::main(&SCENARIO, &CASES)
}

/// The host's source address toward the station network and the route the
/// kernel takes there, as `ip -4 route get` prints it. The route must be an
/// explicit one through a gateway (the router), never the default.
fn host_route(to: Ipv4Addr) -> Result<(Ipv4Addr, String), Failure> {
    let output = std::process::Command::new("ip")
        .args(["-4", "route", "get", &to.to_string()])
        .output()
        .map_err(|error| Failure::Incomplete(format!("ip route get: {error}")))?;
    let route = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let field = |name: &str| {
        let mut words = route.split_whitespace();
        words.find(|word| *word == name).and(words.next())
    };
    let source = field("src")
        .and_then(|source| source.parse().ok())
        .ok_or_else(|| Failure::Incomplete(format!("no source address toward {to}: `{route}`")))?;
    let explicit = std::process::Command::new("ip")
        .args(["-4", "route", "show", STATION_NETWORK])
        .output()
        .map(|output| !String::from_utf8_lossy(&output.stdout).trim().is_empty())
        .unwrap_or(false);
    if field("via").is_none() || !explicit {
        return Err(Failure::Incomplete(format!(
            "the host has no route to {STATION_NETWORK} through the lab router \
             (`ip route add {STATION_NETWORK} via <router LAN address>`, once): `{route}`"
        )));
    }
    Ok((source, route))
}

fn channel() -> Result<u8, Failure> {
    let stand = oer_lab_model::path()
        .and_then(|path| oer_lab_model::Stand::load(&path))
        .map_err(|error| Failure::Incomplete(format!("the stand file: {error}")))?;
    stand
        .router
        .as_ref()
        .and_then(|router| router.channels.first().copied())
        .ok_or_else(|| Failure::Incomplete("the stand file's [router] allows no channel".into()))
}

fn generated(tag: &str) -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
        ^ u128::from(std::process::id());
    format!("{tag}{:016x}", seed as u64)
}

fn run_id() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |elapsed| elapsed.subsec_nanos() | 1)
}

fn status(cx: &mut Cx<'_>) -> Result<Status, Failure> {
    cx.call(GetStatus, CALL)
}

/// The image's address once it has one.
fn device_address(cx: &mut Cx<'_>) -> Result<Ipv4Addr, Failure> {
    status(cx)?
        .ipv4
        .map(|ipv4| Ipv4Addr::from(ipv4.address))
        .ok_or_else(|| Failure::Incomplete("the station has no address: run `path` first".into()))
}

fn cpu(cx: &mut Cx<'_>) -> Result<Load, Failure> {
    cx.call(GetCpu, CALL)
}

/// Each hart's busy share and each executor's work share between two CPU
/// reports. Executors' work spans overlap (preemption), so only a hart's
/// idle time gives its utilisation.
fn utilisation(before: &Load, after: &Load) -> serde_json::Value {
    let window = after.now_us.saturating_sub(before.now_us).max(1) as f64;
    let harts = after
        .harts
        .iter()
        .map(|hart| {
            let idle_before = before
                .harts
                .iter()
                .find(|earlier| earlier.hart == hart.hart)
                .map_or(0, |earlier| earlier.idle_us);
            let idle = hart.idle_us.saturating_sub(idle_before) as f64;
            serde_json::json!({
                "hart": hart.hart,
                "busy_percent": round(100.0 * (1.0 - idle / window)),
            })
        })
        .collect::<Vec<_>>();
    let executors = after
        .executors
        .iter()
        .map(|executor| {
            let work_before = before
                .executors
                .iter()
                .find(|earlier| earlier.executor == executor.executor)
                .map_or(0, |earlier| earlier.work_us);
            serde_json::json!({
                "executor": executor.executor,
                "hart": executor.hart,
                "level": executor.level,
                "work_percent": round(
                    100.0 * executor.work_us.saturating_sub(work_before) as f64 / window,
                ),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "window_ms": round(window / 1000.0), "harts": harts, "executors": executors })
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn mbps(bytes: u64, seconds: f64) -> f64 {
    if seconds <= 0.0 {
        return 0.0;
    }
    round(bytes as f64 * 8.0 / seconds / 1e6)
}

fn rtt(echoes: &traffic::Echoes) -> serde_json::Value {
    let mut sorted = echoes.rtt_us.clone();
    sorted.sort_unstable();
    let at = |share: f64| {
        sorted
            .get(((sorted.len() as f64 - 1.0) * share).round() as usize)
            .map(|us| round(*us as f64 / 1000.0))
    };
    serde_json::json!({
        "probes": echoes.sent,
        "replies": sorted.len(),
        "min_ms": at(0.0),
        "median_ms": at(0.5),
        "p95_ms": at(0.95),
        "max_ms": at(1.0),
    })
}

/// The station joins the router's network, which forwards UDP between it
/// and this host; the route, the addresses and both directions of the path
/// are checked with probe datagrams before any load.
fn path(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let (host, _) = host_route(Ipv4Addr::from(ROUTER_ADDRESS))?;
    let channel = channel()?;
    let (ssid, passphrase) = (generated("oer-load-"), generated("oer-"));
    cx.router(RouterOperation::ApplyStationProfile {
        ssid: ssid.clone(),
        passphrase: passphrase.clone(),
        channel,
        forward_host: Some(host),
    })?;
    let join = Join {
        ssid: heapless::String::try_from(ssid.as_str())
            .map_err(|_| Failure::Incomplete("the generated SSID is too long".into()))?,
        passphrase: heapless::String::try_from(passphrase.as_str())
            .map_err(|_| Failure::Incomplete("the generated passphrase is too long".into()))?,
        channel,
    };
    let Joined::Linked {
        bssid,
        channel: linked,
    } = cx.call(join, JOIN_TIMEOUT)?
    else {
        return Err(Failure::Fail("the station did not join".into()));
    };
    let started = Instant::now();
    let (address, ipv4) = loop {
        let status = status(cx)?;
        if let Some(ipv4) = status.ipv4 {
            break (status.address, ipv4);
        }
        if started.elapsed() >= DHCP_TIMEOUT {
            return Err(Failure::Fail(format!("no DHCP address: {status:?}")));
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let device = Ipv4Addr::from(ipv4.address);
    let (source, route) = match host_route(device) {
        Ok((source, device_route)) if source == host => (source, device_route),
        Ok((source, _)) => {
            return Err(Failure::Fail(format!(
                "the host reaches the device from {source}, the router forwards {host}"
            )));
        }
        Err(failure) => return Err(failure),
    };
    // Both ways: the host's probes come back echoed, the image's datagrams
    // reach the host.
    let run = run_id();
    let endpoint = traffic::Endpoint::open(SocketAddrV4::new(source, 0), run)
        .map_err(|error| Failure::Incomplete(format!("the host socket: {error}")))?;
    let host_port = endpoint
        .socket
        .local_addr()
        .map(|local| local.port())
        .unwrap_or(0);
    let started = cx.call(
        Start {
            run,
            port: DEVICE_PORT,
            transmit: Some(Transmit {
                peer: source.octets(),
                peer_port: host_port,
                payload: PAYLOAD as u16,
                rate_bps: 100_000,
                delay_ms: 0,
                duration_ms: 1000,
            }),
        },
        CALL,
    )?;
    check(
        started == Started::Running,
        format!("the image's load: {started:?}"),
    )?;
    let stop = AtomicBool::new(false);
    let peer = SocketAddrV4::new(device, DEVICE_PORT);
    let sent = std::thread::scope(|scope| {
        let probes = scope.spawn(|| traffic::probe(&endpoint, peer, run, PROBE_PERIOD, &stop));
        std::thread::sleep(Duration::from_millis(1500));
        stop.store(true, Ordering::Relaxed);
        probes.join().unwrap_or(0)
    });
    std::thread::sleep(DRAIN);
    let collected = cx.call(Collect { run }, CALL)?;
    let mut host = endpoint.close();
    host.echoes.sent = sent;
    cx.measure(
        "path",
        serde_json::json!({
            "topology": "host -> Ethernet -> OpenWrt AP -> Wi-Fi -> DUT station",
            "host": source.to_string(),
            "host_route": route,
            "device": device.to_string(),
            "device_mac": mac(address),
            "bssid": mac(bssid),
            "channel": linked,
            "gateway": ipv4.gateway.map(|gateway| Ipv4Addr::from(gateway).to_string()),
            "rtt": rtt(&host.echoes),
            "device_to_host_datagrams": host.load.datagrams,
            "device_report": format!("{collected:?}"),
        }),
    );
    let Collected::Report(report) = collected else {
        return Err(Failure::Fail(format!("the image's report: {collected:?}")));
    };
    check(
        linked == channel,
        format!("linked on channel {linked}, the network is on {channel}"),
    )?;
    check(
        !host.echoes.rtt_us.is_empty() && report.received.echoed > 0,
        format!(
            "host → device: {} probes, {} echoes back, the image echoed {}",
            host.echoes.sent,
            host.echoes.rtt_us.len(),
            report.received.echoed
        ),
    )?;
    check(
        host.load.datagrams > 0 && report.sent.datagrams > 0,
        format!(
            "device → host: the image sent {}, the host received {}",
            report.sent.datagrams, host.load.datagrams
        ),
    )
}

fn mac(address: [u8; 6]) -> String {
    address
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Which way a level's load goes, from the image's side.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Rx,
    Tx,
    RxTx,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Rx => "rx",
            Self::Tx => "tx",
            Self::RxTx => "rx-tx",
        }
    }
}

fn rx(cx: &mut Cx<'_>) -> Result<(), Failure> {
    levels(cx, Mode::Rx)
}

fn tx(cx: &mut Cx<'_>) -> Result<(), Failure> {
    levels(cx, Mode::Tx)
}

fn rx_tx(cx: &mut Cx<'_>) -> Result<(), Failure> {
    levels(cx, Mode::RxTx)
}

fn levels(cx: &mut Cx<'_>, mode: Mode) -> Result<(), Failure> {
    let device = device_address(cx)?;
    let (host, _) = host_route(device)?;
    for total in LEVELS_MBPS {
        level(cx, mode, device, host, total)?;
    }
    Ok(())
}

/// One level: `total_mbps` offered, split over the mode's directions, for
/// [`DURATION`], with RTT probes throughout.
fn level(
    cx: &mut Cx<'_>,
    mode: Mode,
    device: Ipv4Addr,
    host: Ipv4Addr,
    total_mbps: u64,
) -> Result<(), Failure> {
    let directions = if mode == Mode::RxTx { 2 } else { 1 };
    let each_bps = total_mbps * 1_000_000 / directions;
    let run = run_id();
    let endpoint = traffic::Endpoint::open(SocketAddrV4::new(host, 0), run)
        .map_err(|error| Failure::Incomplete(format!("the host socket: {error}")))?;
    let host_port = endpoint
        .socket
        .local_addr()
        .map(|local| local.port())
        .unwrap_or(0);
    let transmit = (mode != Mode::Rx).then(|| Transmit {
        peer: host.octets(),
        peer_port: host_port,
        payload: PAYLOAD as u16,
        rate_bps: u32::try_from(each_bps).unwrap_or(u32::MAX),
        delay_ms: DEVICE_DELAY_MS,
        duration_ms: u32::try_from(DURATION.as_millis()).unwrap_or(u32::MAX),
    });
    let before = cpu(cx)?;
    let started = cx.call(
        Start {
            run,
            port: DEVICE_PORT,
            transmit,
        },
        CALL,
    )?;
    check(
        started == Started::Running,
        format!("the image's load: {started:?}"),
    )?;
    let peer = SocketAddrV4::new(device, DEVICE_PORT);
    let stop = AtomicBool::new(false);
    let (sent, probes) = std::thread::scope(|scope| {
        let probes = scope.spawn(|| traffic::probe(&endpoint, peer, run, PROBE_PERIOD, &stop));
        let sent = if mode == Mode::Tx {
            std::thread::sleep(DURATION + Duration::from_millis(u64::from(DEVICE_DELAY_MS)));
            traffic::Sent::default()
        } else {
            std::thread::sleep(Duration::from_millis(u64::from(DEVICE_DELAY_MS)));
            traffic::send(&endpoint, peer, run, PAYLOAD, each_bps, DURATION)
        };
        stop.store(true, Ordering::Relaxed);
        (sent, probes.join().unwrap_or(0))
    });
    std::thread::sleep(DRAIN);
    let collected = cx.call(Collect { run }, CALL)?;
    let after = cpu(cx)?;
    let mut received = endpoint.close();
    received.echoes.sent = probes;
    let Collected::Report(report) = collected else {
        return Err(Failure::Fail(format!("the image's report: {collected:?}")));
    };
    cx.measure(
        format!("{} {total_mbps} Mbit/s", mode.name()),
        measurement(mode, total_mbps, &sent, &received, &report, &before, &after),
    );
    Ok(())
}

fn span_seconds(first: Option<u64>, last: Option<u64>) -> f64 {
    match (first, last) {
        (Some(first), Some(last)) => last.saturating_sub(first) as f64 / 1e6,
        _ => 0.0,
    }
}

fn measurement(
    mode: Mode,
    total_mbps: u64,
    sent: &traffic::Sent,
    received: &traffic::Received,
    report: &Report,
    before: &Load,
    after: &Load,
) -> serde_json::Value {
    let directions = if mode == Mode::RxTx { 2 } else { 1 };
    let offered_each = round(total_mbps as f64 / directions as f64);
    let mut value = serde_json::json!({
        "offered_total_mbps": total_mbps,
        "payload_bytes": PAYLOAD,
        "rtt": rtt(&received.echoes),
        "cpu": utilisation(before, after),
    });
    let mut sides = serde_json::Map::new();
    if mode != Mode::Tx {
        let device = &report.received;
        let expected = device.highest.map_or(0, |highest| highest + 1);
        sides.insert("rx".to_owned(), serde_json::json!({
            "offered_mbps": offered_each,
            "host_sent_mbps": mbps(sent.bytes, sent.elapsed.as_secs_f64()),
            "host_sent_datagrams": sent.datagrams,
            "host_send_failed": sent.failed,
            "device_received_mbps": mbps(device.bytes, span_seconds(device.first_us, device.last_us)),
            "device_received_datagrams": device.datagrams,
            "lost": sent.datagrams.saturating_sub(device.datagrams),
            "lost_below_highest": device.lost,
            "lost_percent": round(100.0 * sent.datagrams.saturating_sub(device.datagrams) as f64
                / sent.datagrams.max(1) as f64),
            "reordered": device.reordered,
            "duplicates": device.duplicates,
            "foreign": device.foreign,
            "highest_plus_one": expected,
        }));
    }
    if mode != Mode::Rx {
        let host = &received.load;
        let device = &report.sent;
        let seconds = match (host.first, host.last) {
            (Some(first), Some(last)) => last.saturating_duration_since(first).as_secs_f64(),
            _ => 0.0,
        };
        sides.insert("tx".to_owned(), serde_json::json!({
            "offered_mbps": offered_each,
            "device_sent_mbps": mbps(device.bytes, span_seconds(device.first_us, device.last_us)),
            "device_sent_datagrams": device.datagrams,
            "device_send_refused": device.refused,
            "device_send_failed": device.failed,
            "host_received_mbps": mbps(host.bytes, seconds),
            "host_received_datagrams": host.datagrams,
            "lost": device.datagrams.saturating_sub(host.datagrams),
            "lost_percent": round(100.0 * device.datagrams.saturating_sub(host.datagrams) as f64
                / device.datagrams.max(1) as f64),
            "lost_below_highest": host.lost(),
            "reordered": host.reordered,
            "duplicates": host.duplicates,
        }));
    }
    if let Some(object) = value.as_object_mut() {
        object.extend(sides);
    }
    value
}
