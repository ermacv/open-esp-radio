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
use oer_services_load::{Collect, Collected, Load, Report, Start, Started, Transmit};
use oer_services_wifi_station::{GetStatus, Join, Joined, Status};

const SCENARIO: Scenario = Scenario::new(
    "wifi-load",
    "esp32s31-wifi-station",
    &[
        oer_services_wifi_station::Request::KEYS,
        oer_services_load::Request::KEYS,
    ],
)
.using_router()
.using_air_exclusively();

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
/// offers half each way. `OER_WIFI_LOAD_MBPS` (comma-separated) replaces
/// them, e.g. to offer more than HT20 carries.
const LEVELS_MBPS: [u64; 4] = [10, 30, 50, 65];
const LEVELS_ENV: &str = "OER_WIFI_LOAD_MBPS";
/// Each level's measuring window, the time before it opens and the drain
/// after it (the image's run, `oer_services_load::Start`).
const DURATION_MS: u32 = 5000;
const DELAY_MS: u32 = 300;
const DRAIN_MS: u32 = 500;
/// RTT probes during a level.
const PROBE_PERIOD: Duration = Duration::from_millis(50);
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);
const DHCP_TIMEOUT: Duration = Duration::from_secs(30);
const CALL: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    oer_hil_harness::main(&SCENARIO, &CASES)
}

/// The route the host takes to an address of the station network, as
/// `ip -4 route get` shows it.
struct Route {
    source: Ipv4Addr,
    device: String,
    gateway: Ipv4Addr,
    line: String,
}

/// The host's route to `to`: it must go through the router's LAN address
/// the stand file names, over a wired interface (one hop over the air), and
/// leave from one source address.
fn host_route(to: Ipv4Addr) -> Result<Route, Failure> {
    let output = std::process::Command::new("ip")
        .args(["-4", "route", "get", &to.to_string()])
        .output()
        .map_err(|error| Failure::Incomplete(format!("ip route get: {error}")))?;
    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let field = |name: &str| {
        let mut words = line.split_whitespace();
        words.find(|word| *word == name).and(words.next())
    };
    let route = Route {
        source: field("src")
            .and_then(|source| source.parse().ok())
            .ok_or_else(|| {
                Failure::Incomplete(format!("no source address toward {to}: `{line}`"))
            })?,
        device: field("dev").unwrap_or_default().to_owned(),
        gateway: field("via")
            .and_then(|via| via.parse().ok())
            .ok_or_else(|| {
                Failure::Incomplete(format!(
                    "the host reaches {to} without a gateway, not through the lab router: `{line}`"
                ))
            })?,
        line: line.clone(),
    };
    let lan_address = stand()?
        .router
        .and_then(|router| router.lan_address)
        .ok_or_else(|| {
            Failure::Incomplete(
                "the stand file's [router] names no lan_address: the router's address on the \
                 wired network the host reaches it by"
                    .into(),
            )
        })?;
    if route.gateway != lan_address {
        return Err(Failure::Incomplete(format!(
            "the host reaches {to} through {}, not the lab router {lan_address} \
             (`ip route add {STATION_NETWORK} via {lan_address}`, once): `{line}`",
            route.gateway
        )));
    }
    let wireless = std::path::Path::new("/sys/class/net")
        .join(&route.device)
        .join("wireless");
    if route.device.is_empty() || wireless.exists() {
        return Err(Failure::Incomplete(format!(
            "the host reaches {to} over `{}`, not a wired interface: `{line}`",
            route.device
        )));
    }
    Ok(route)
}

fn stand() -> Result<oer_lab_model::Stand, Failure> {
    oer_lab_model::path()
        .and_then(|path| oer_lab_model::Stand::load(&path))
        .map_err(|error| Failure::Incomplete(format!("the stand file: {error}")))
}

fn channel() -> Result<u8, Failure> {
    stand()?
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

/// The report of `run`, once the image ended it.
fn collect(cx: &mut Cx<'_>, run: u32) -> Result<Report, Failure> {
    let started = Instant::now();
    loop {
        match cx.call(Collect { run }, CALL)? {
            Collected::Report(report) => return Ok(report),
            Collected::Running if started.elapsed() < Duration::from_secs(10) => {
                std::thread::sleep(Duration::from_millis(100));
            }
            other => {
                return Err(Failure::Fail(format!("the image's report: {other:?}")));
            }
        }
    }
}

/// The highest total a level may offer: far above any 2.4 GHz link, and
/// each direction's rate stays a `u32` of bit/s.
const MAX_LEVEL_MBPS: u64 = 1000;

/// The levels to offer: [`LEVELS_MBPS`], or `OER_WIFI_LOAD_MBPS`, each 1 to
/// [`MAX_LEVEL_MBPS`].
fn levels_mbps() -> Result<Vec<u64>, Failure> {
    match std::env::var(LEVELS_ENV) {
        Ok(text) => parse_levels(&text),
        Err(_) => Ok(LEVELS_MBPS.to_vec()),
    }
}

fn parse_levels(text: &str) -> Result<Vec<u64>, Failure> {
    text.split(',')
        .map(|level| {
            level
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|level| (1..=MAX_LEVEL_MBPS).contains(level))
        })
        .collect::<Option<Vec<_>>>()
        .filter(|levels| !levels.is_empty())
        .ok_or_else(|| {
            Failure::Incomplete(format!(
                "{LEVELS_ENV}=`{text}` is no list of 1 to {MAX_LEVEL_MBPS} Mbit/s"
            ))
        })
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
    let host = host_route(Ipv4Addr::from(ROUTER_ADDRESS))?.source;
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
    let route = host_route(device)?;
    let source = route.source;
    if source != host {
        return Err(Failure::Fail(format!(
            "the host reaches the device from {source}, the router forwards {host}"
        )));
    }
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
            delay_ms: 0,
            duration_ms: 1000,
            drain_ms: DRAIN_MS,
            transmit: Some(Transmit {
                peer: source.octets(),
                peer_port: host_port,
                payload: PAYLOAD as u16,
                rate_bps: 100_000,
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
    let report = collect(cx, run)?;
    let mut host = endpoint.close();
    host.echoes.sent = sent;
    cx.measure(
        "path",
        serde_json::json!({
            "topology": "host -> Ethernet -> OpenWrt AP -> Wi-Fi -> DUT station",
            "host": source.to_string(),
            "host_interface": route.device,
            "host_gateway": route.gateway.to_string(),
            "host_route": route.line,
            "device": device.to_string(),
            "device_mac": mac(address),
            "bssid": mac(bssid),
            "channel": linked,
            "gateway": ipv4.gateway.map(|gateway| Ipv4Addr::from(gateway).to_string()),
            "rtt": rtt(&host.echoes),
            "device_to_host_datagrams": host.load.datagrams,
            "device_echoed": report.received.echoed,
            "device_sent_datagrams": report.sent.datagrams,
        }),
    );
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
    )?;
    // The image's CPU with the station linked and no load: the baseline of
    // every level's CPU.
    let idle = run_id();
    let started = cx.call(
        Start {
            run: idle,
            port: DEVICE_PORT,
            delay_ms: 0,
            duration_ms: DURATION_MS,
            drain_ms: 0,
            transmit: None,
        },
        CALL,
    )?;
    check(
        started == Started::Running,
        format!("the image's idle run: {started:?}"),
    )?;
    std::thread::sleep(Duration::from_millis(u64::from(DURATION_MS)));
    let idle = collect(cx, idle)?;
    cx.measure("idle cpu", utilisation(&idle.cpu_start, &idle.cpu_end));
    Ok(())
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
    let host = host_route(device)?.source;
    for total in levels_mbps()? {
        level(cx, mode, device, host, total)?;
    }
    Ok(())
}

/// One level: `total_mbps` offered, split over the mode's directions, for
/// the window, with RTT probes from the start to the end of the drain.
fn level(
    cx: &mut Cx<'_>,
    mode: Mode,
    device: Ipv4Addr,
    host: Ipv4Addr,
    total_mbps: u64,
) -> Result<(), Failure> {
    let directions = if mode == Mode::RxTx { 2 } else { 1 };
    let each_bps = total_mbps
        .checked_mul(1_000_000)
        .map(|bps| bps / directions)
        .filter(|bps| u32::try_from(*bps).is_ok())
        .ok_or_else(|| Failure::Incomplete(format!("{total_mbps} Mbit/s is out of range")))?;
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
        rate_bps: u32::try_from(each_bps).unwrap_or(0),
    });
    let started = cx.call(
        Start {
            run,
            port: DEVICE_PORT,
            delay_ms: DELAY_MS,
            duration_ms: DURATION_MS,
            drain_ms: DRAIN_MS,
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
    let window = Duration::from_millis(u64::from(DURATION_MS));
    let (sent, probes) = std::thread::scope(|scope| {
        // The host's window starts with the image's, as near as the HIL
        // answer's latency allows; the image counts only its own window.
        // The probes go within the window only; their late replies are
        // still read during the drain.
        std::thread::sleep(Duration::from_millis(u64::from(DELAY_MS)));
        let probes = scope.spawn(|| traffic::probe(&endpoint, peer, run, PROBE_PERIOD, &stop));
        let sent = if mode == Mode::Tx {
            std::thread::sleep(window);
            traffic::Sent::default()
        } else {
            traffic::send(&endpoint, peer, run, PAYLOAD, each_bps, window)
        };
        stop.store(true, Ordering::Relaxed);
        let probes = probes.join().unwrap_or(0);
        std::thread::sleep(Duration::from_millis(u64::from(DRAIN_MS)));
        (sent, probes)
    });
    let report = collect(cx, run)?;
    let mut received = endpoint.close();
    received.echoes.sent = probes;
    cx.measure(
        format!("{} {total_mbps} Mbit/s", mode.name()),
        measurement(mode, total_mbps, &sent, &received, &report),
    );
    Ok(())
}

/// One level's measurement. Throughput is over the measuring window: the
/// image's window for what it received and sent, the host's own sending
/// time for what the host sent, the image's window for what the host
/// received (the image sends only within it). CPU is the image's, at the
/// window's two ends.
fn measurement(
    mode: Mode,
    total_mbps: u64,
    sent: &traffic::Sent,
    received: &traffic::Received,
    report: &Report,
) -> serde_json::Value {
    let directions = if mode == Mode::RxTx { 2 } else { 1 };
    let offered_each = round(total_mbps as f64 / directions as f64);
    let window = report.window_end_us.saturating_sub(report.window_start_us) as f64 / 1e6;
    let mut value = serde_json::json!({
        "offered_total_mbps": total_mbps,
        "payload_bytes": PAYLOAD,
        "window_s": round(window),
        "rtt": rtt(&received.echoes),
        "cpu": utilisation(&report.cpu_start, &report.cpu_end),
    });
    let mut sides = serde_json::Map::new();
    if mode != Mode::Tx {
        let device = &report.received;
        sides.insert(
            "rx".to_owned(),
            serde_json::json!({
                "offered_mbps": offered_each,
                "host_sent_mbps": mbps(sent.bytes, sent.elapsed.as_secs_f64()),
                "host_sent_datagrams": sent.datagrams,
                "host_send_failed": sent.failed,
                "device_received_mbps": mbps(device.window_bytes, window),
                "device_received_datagrams": device.datagrams,
                "device_received_in_window": device.window_datagrams,
                "lost": sent.datagrams.saturating_sub(device.datagrams),
                "lost_percent": percent(sent.datagrams.saturating_sub(device.datagrams), sent.datagrams),
                "lost_below_highest": device.lost,
                "reordered": device.reordered,
                "duplicates": device.duplicates,
                "untracked": device.untracked,
                "foreign": device.foreign,
            }),
        );
    }
    if mode != Mode::Rx {
        let host = &received.load;
        let device = &report.sent;
        sides.insert(
            "tx".to_owned(),
            serde_json::json!({
                "offered_mbps": offered_each,
                "device_sent_mbps": mbps(device.bytes, window),
                "device_sent_datagrams": device.datagrams,
                "device_send_refused": device.refused,
                "device_send_missed": device.missed,
                "device_send_failed": device.failed,
                "host_received_mbps": mbps(host.bytes, window),
                "host_received_datagrams": host.datagrams,
                "lost": device.datagrams.saturating_sub(host.datagrams),
                "lost_percent": percent(device.datagrams.saturating_sub(host.datagrams), device.datagrams),
                "lost_below_highest": host.lost(),
                "reordered": host.reordered,
                "duplicates": host.duplicates,
                "untracked": host.untracked,
            }),
        );
    }
    if let Some(object) = value.as_object_mut() {
        object.extend(sides);
    }
    value
}

fn percent(part: u64, whole: u64) -> f64 {
    round(100.0 * part as f64 / whole.max(1) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throughput_is_over_the_window_not_between_the_first_and_last_datagram() {
        let report = Report {
            window_start_us: 1_000_000,
            window_end_us: 6_000_000,
            received: oer_services_load::Received {
                // 65 Mbit in the window's first second, nothing after.
                window_bytes: 65_000_000 / 8,
                datagrams: 5804,
                first_us: Some(1_000_000),
                last_us: Some(2_000_000),
                ..Default::default()
            },
            ..Default::default()
        };
        let sent = traffic::Sent {
            datagrams: 5804,
            bytes: 65_000_000 / 8,
            failed: 0,
            elapsed: Duration::from_secs(5),
        };
        let value = measurement(Mode::Rx, 65, &sent, &traffic::Received::default(), &report);
        assert_eq!(value["window_s"], 5.0);
        assert_eq!(value["rx"]["device_received_mbps"], 13.0);
        assert!(value.get("tx").is_none());
    }

    #[test]
    fn a_hart_s_busy_share_is_its_window_less_its_idle_time() {
        let at = |now_us, idle_us, work_us| Load {
            now_us,
            harts: [oer_services_load::HartLoad { hart: 0, idle_us }]
                .into_iter()
                .collect(),
            executors: [oer_services_load::ExecutorLoad {
                executor: 1,
                hart: 0,
                level: 2,
                work_us,
            }]
            .into_iter()
            .collect(),
        };
        let value = utilisation(
            &at(1_000_000, 400_000, 0),
            &at(2_000_000, 1_000_000, 300_000),
        );
        assert_eq!(value["harts"][0]["busy_percent"], 40.0);
        assert_eq!(value["executors"][0]["work_percent"], 30.0);
    }

    #[test]
    fn a_level_list_holds_only_representable_totals() {
        assert_eq!(LEVELS_MBPS.iter().max(), Some(&65));
        assert_eq!(parse_levels("10, 65,100").ok(), Some(vec![10, 65, 100]));
        for refused in ["", "0", "1001", "18446744073709551615", "10,x"] {
            assert!(parse_levels(refused).is_err(), "{refused}");
        }
        assert!(u32::try_from(MAX_LEVEL_MBPS * 1_000_000).is_ok());
    }
}
