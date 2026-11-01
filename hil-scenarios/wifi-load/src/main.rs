//! The HIL Wi-Fi station load scenario. The image's station joins the lab
//! router's isolated network, HT 40 MHz wide, which forwards UDP between it
//! and this host (wired to the router: one hop over the air). The host sends and receives
//! the load; the image counts its end and reports its CPU over the HIL link.
//! RX and TX are named from the image's side. Every level of every mode is
//! measured and reported (`target/hil/results/`): the measurements never
//! decide a case, only a path or an exchange that does not work does.
//! Each window also records what the link ran ([`link`]): the access
//! point's view of the station and the image's rates each way.
//! With `OER_WIFI_LOAD_PROFILE_US` each level also samples where the
//! image's hart spends its window ([`profile`]).
#![forbid(unsafe_code)]

mod link;
mod profile;
mod traffic;

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    process::ExitCode,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use oer_hil_agent::base::Service as _;
use oer_hil_harness::{Case, Cx, Failure, RouterChannelWidth, RouterOperation, Scenario, check};
use oer_services_load::{
    CacheGeometry, CacheLevel, Collect, CollectCounters, CollectLink, CollectParts, Collected,
    Counters, CountersCollected, DataBus, InstructionBus, Link, LinkCollected, Load, MAX_PARTS,
    PartWindow, PartsCollected, Report, Start, Started, Transmit,
};
use oer_services_wifi_station::{GetStatus, Join, Joined, Leave, Left, Status};

/// The image the scenario runs.
const IMAGE: &str = "esp32s31-wifi-station";

const SCENARIO: Scenario = Scenario::new(
    "wifi-load",
    IMAGE,
    &[
        oer_services_wifi_station::Request::KEYS,
        oer_services_load::Request::KEYS,
    ],
)
.using_router()
.using_air_exclusively();

const CASES: [Case; 5] = [
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
    // Last: the image serves no station after a leave.
    Case {
        name: "stop",
        run: stop_under_load,
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
/// them, e.g. to offer more than these levels.
const LEVELS_MBPS: [u64; 4] = [10, 30, 50, 65];
const LEVELS_ENV: &str = "OER_WIFI_LOAD_MBPS";
/// Each level's measuring window, the time before it opens and the drain
/// after it (the image's run, `oer_services_load::Start`).
const DURATION_MS: u32 = 5000;
const DELAY_MS: u32 = 300;
const DRAIN_MS: u32 = 500;
/// RTT probes during a level. `OER_WIFI_LOAD_PROBES=off` sends none (`on`
/// is the default), to measure a level without the probes' datagrams.
const PROBE_PERIOD: Duration = Duration::from_millis(50);
const PROBES_ENV: &str = "OER_WIFI_LOAD_PROBES";
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

/// The report of `run` and its parts' window, once the image ended it.
/// What the image kept of one run.
struct Ended {
    report: Report,
    parts: Vec<PartWindow>,
    counters: Counters,
    link: Link,
}

fn collect(cx: &mut Cx<'_>, run: u32) -> Result<Ended, Failure> {
    let started = Instant::now();
    let report = loop {
        match cx.call(Collect { run }, CALL)? {
            Collected::Report(report) => break report,
            Collected::Running if started.elapsed() < Duration::from_secs(10) => {
                std::thread::sleep(Duration::from_millis(100));
            }
            other => {
                return Err(Failure::Fail(format!("the image's report: {other:?}")));
            }
        }
    };
    let mut parts = Vec::new();
    loop {
        let from = u8::try_from(parts.len())
            .map_err(|_| Failure::Fail("the image names too many parts".to_owned()))?;
        match cx.call(CollectParts { run, from }, CALL)? {
            PartsCollected::Report(page) => {
                let full = page.parts.len() == MAX_PARTS;
                parts.extend(page.parts);
                if !full {
                    break;
                }
            }
            other => return Err(Failure::Fail(format!("the image's parts: {other:?}"))),
        }
    }
    let counters = match cx.call(CollectCounters { run }, CALL)? {
        CountersCollected::Report(counters) => counters,
        other => return Err(Failure::Fail(format!("the image's counters: {other:?}"))),
    };
    match cx.call(CollectLink { run }, CALL)? {
        LinkCollected::Report(link) => Ok(Ended {
            report,
            parts,
            counters,
            link,
        }),
        other => Err(Failure::Fail(format!("the image's link: {other:?}"))),
    }
}

/// What the link ran over a window: the access point's view between
/// `before` and `after`, `span` apart, and the image's counts in `ended`;
/// its one-line summary is printed as `name`'s.
fn link_measurement(
    name: &str,
    before: &link::Reading,
    after: &link::Reading,
    span: Duration,
    ended: &Ended,
) -> serde_json::Value {
    let router = link::router_window(before, after, span);
    let device = link::device(&ended.link);
    println!("  link {name}: {}", link::summary(&router, &device));
    serde_json::json!({ "router": router, "device": device })
}

/// The highest total a level may offer: far above any 2.4 GHz link, and
/// each direction's rate stays a `u32` of bit/s.
const MAX_LEVEL_MBPS: u64 = 1000;

/// The channel width the router's profile asks for: HT40, or the
/// `OER_WIFI_LOAD_WIDTH` it names (`ht20` or `ht40`).
const WIDTH_ENV: &str = "OER_WIFI_LOAD_WIDTH";

fn width() -> Result<RouterChannelWidth, Failure> {
    match std::env::var(WIDTH_ENV) {
        Err(_) => Ok(RouterChannelWidth::Ht40),
        Ok(text) => parse_width(&text),
    }
}

fn parse_width(text: &str) -> Result<RouterChannelWidth, Failure> {
    match text.trim() {
        "ht20" => Ok(RouterChannelWidth::Ht20),
        "ht40" => Ok(RouterChannelWidth::Ht40),
        other => Err(Failure::Incomplete(format!(
            "{WIDTH_ENV}=`{other}` is neither `ht20` nor `ht40`"
        ))),
    }
}

/// Whether the levels send RTT probes: yes, unless `OER_WIFI_LOAD_PROBES`
/// says `off`.
fn probes() -> Result<bool, Failure> {
    match std::env::var(PROBES_ENV) {
        Err(_) => Ok(true),
        Ok(text) => parse_probes(&text),
    }
}

fn parse_probes(text: &str) -> Result<bool, Failure> {
    match text.trim() {
        "on" => Ok(true),
        "off" => Ok(false),
        other => Err(Failure::Incomplete(format!(
            "{PROBES_ENV}=`{other}` is neither `on` nor `off`"
        ))),
    }
}

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

/// The run's measuring window as the image kept it.
fn window_us(report: &Report) -> u64 {
    report.window_end_us.saturating_sub(report.window_start_us)
}

/// Each named part's polls within a window of `window_us`. A poll's time
/// includes its preemption, so nested parts and the parts of different
/// executors overlap and their shares do not add up. The share counts the
/// window's time inside polls, an open one's included; the mean counts only
/// the polls that ended, whole.
fn polls(parts: &[PartWindow], window_us: u64) -> serde_json::Value {
    let window = window_us.max(1) as f64;
    parts
        .iter()
        .map(|part| {
            serde_json::json!({
                "part": part.name.as_str(),
                "polls": part.polls,
                "polls_per_s": round(f64::from(part.polls) * 1e6 / window),
                "poll_time_percent": round(100.0 * f64::from(part.poll_us) / window),
                "mean_poll_us": round(f64::from(part.ended_poll_us) / f64::from(part.polls.max(1))),
                "longest_poll_us": part.longest_poll_us,
            })
        })
        .collect()
}

/// The window's counters: the image's measured hart's cycles and retired
/// instructions with their rate per microsecond (MHz) and per cycle, what
/// the network bridge did with the station's frames, the frames the IP
/// stack took with the hart's instructions over them, and the L1 cache's
/// accesses by bus ([`cache`]), and the receive path's counts as the image
/// took them: the S31 driver's and the station service's receive
/// decisions, each the window's difference. The hart's counts include all
/// its work, interrupts and the load service's own included.
fn counters(counters: &Counters, window_us: u64) -> serde_json::Value {
    let window = window_us.max(1) as f64;
    let cycles = counters.cycles as f64;
    let instructions = counters.instructions as f64;
    serde_json::json!({
        "cycles": counters.cycles,
        "instructions": counters.instructions,
        "cycles_per_us": round(cycles / window),
        "instructions_per_cycle": (counters.cycles != 0).then(|| round(instructions / cycles)),
        "bridge": {
            "received": counters.bridge_received,
            "refused_pool": counters.bridge_refused_pool,
            "refused_queue": counters.bridge_refused_queue,
            "refused_other": counters.bridge_refused_other,
            "copied": counters.bridge_copied,
        },
        "stack_received": counters.stack_received,
        "instructions_per_stack_frame": (counters.stack_received != 0)
            .then(|| round(instructions / f64::from(counters.stack_received))),
        "cache": cache(counters),
        "rx": counters.rx,
    })
}

/// What the window's instructions and stack frames rate a cache count by.
struct Per {
    instructions: u64,
    stack_frames: u32,
}

impl Per {
    /// `count` per 1000 of the window's instructions and per stack frame,
    /// to four decimals; null without a denominator or with an overflowed
    /// bus.
    fn rates(&self, name: &str, count: u32, overflowed: bool) -> [(String, serde_json::Value); 2] {
        let exact = !overflowed;
        let count = f64::from(count);
        [
            (
                format!("{name}_per_1000_instructions"),
                (exact && self.instructions != 0)
                    .then(|| fine(1000.0 * count / self.instructions as f64))
                    .into(),
            ),
            (
                format!("{name}_per_stack_frame"),
                (exact && self.stack_frames != 0)
                    .then(|| fine(count / f64::from(self.stack_frames)))
                    .into(),
            ),
        ]
    }
}

/// The L1 cache's counts of each bus within the window, with each bus's
/// misses as a share of its hits and misses, and its misses and next-level
/// reads (and a data bus's next-level writes) per 1000 of the serving
/// hart's instructions and per frame the IP stack took. An overflowed
/// bus's counts are not exact: its derived values are null.
fn cache(counters: &Counters) -> serde_json::Value {
    let per = Per {
        instructions: counters.instructions,
        stack_frames: counters.stack_received,
    };
    let cache = &counters.cache;
    serde_json::json!({
        "ibus0": instruction_bus(&cache.ibus0, &per),
        "ibus1": instruction_bus(&cache.ibus1, &per),
        "dbus0": data_bus(&cache.dbus0, &per),
        "dbus1": data_bus(&cache.dbus1, &per),
    })
}

fn instruction_bus(bus: &InstructionBus, per: &Per) -> serde_json::Value {
    let mut value = serde_json::json!({
        "hits": bus.hits,
        "misses": bus.misses,
        "conflicts": bus.conflicts,
        "next_level_reads": bus.next_level_reads,
        "overflowed": bus.overflowed,
        "miss_percent": miss_percent(bus.hits, bus.misses, bus.overflowed),
    });
    if let Some(object) = value.as_object_mut() {
        object.extend(per.rates("misses", bus.misses, bus.overflowed));
        object.extend(per.rates("next_level_reads", bus.next_level_reads, bus.overflowed));
    }
    value
}

fn data_bus(bus: &DataBus, per: &Per) -> serde_json::Value {
    let mut value = serde_json::json!({
        "hits": bus.hits,
        "misses": bus.misses,
        "conflicts": bus.conflicts,
        "next_level_reads": bus.next_level_reads,
        "next_level_writes": bus.next_level_writes,
        "overflowed": bus.overflowed,
        "miss_percent": miss_percent(bus.hits, bus.misses, bus.overflowed),
    });
    if let Some(object) = value.as_object_mut() {
        object.extend(per.rates("misses", bus.misses, bus.overflowed));
        object.extend(per.rates("next_level_reads", bus.next_level_reads, bus.overflowed));
        object.extend(per.rates("next_level_writes", bus.next_level_writes, bus.overflowed));
    }
    value
}

/// Misses as a share of hits and misses; null without either or when the
/// bus overflowed.
fn miss_percent(hits: u32, misses: u32, overflowed: bool) -> Option<f64> {
    let accesses = u64::from(hits) + u64::from(misses);
    (!overflowed && accesses != 0).then(|| percent(u64::from(misses), accesses))
}

/// The caches as the image configured them.
fn cache_geometry(geometry: &CacheGeometry) -> serde_json::Value {
    let level = |level: &CacheLevel| {
        serde_json::json!({
            "size_bytes": level.size_bytes,
            "line_bytes": level.line_bytes,
            "ways": level.ways,
        })
    };
    serde_json::json!({
        "l1_icache": level(&geometry.l1_icache),
        "l1_dcache": level(&geometry.l1_dcache),
        "l2": level(&geometry.l2),
        "l2_bypassed": geometry.l2_bypassed,
    })
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// `value` to four decimals: rates far below one stay distinct.
fn fine(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
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
        width: width()?,
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
            profile: None,
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
    let report = collect(cx, run)?.report;
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
    let station = mac(address);
    let before = link::read(cx, &station);
    let since = Instant::now();
    let idle = run_id();
    let started = cx.call(
        Start {
            run: idle,
            port: DEVICE_PORT,
            delay_ms: 0,
            duration_ms: DURATION_MS,
            drain_ms: 0,
            transmit: None,
            profile: None,
        },
        CALL,
    )?;
    check(
        started == Started::Running,
        format!("the image's idle run: {started:?}"),
    )?;
    std::thread::sleep(Duration::from_millis(u64::from(DURATION_MS)));
    let after = link::read(cx, &station);
    let span = since.elapsed();
    let idle = collect(cx, idle)?;
    let window = window_us(&idle.report);
    let mut cpu = utilisation(&idle.report.cpu_start, &idle.report.cpu_end);
    if let Some(object) = cpu.as_object_mut() {
        object.insert("parts".to_owned(), polls(&idle.parts, window));
        object.insert("counters".to_owned(), counters(&idle.counters, window));
    }
    cx.measure("idle cpu", cpu);
    cx.measure(
        "cache geometry",
        cache_geometry(&idle.counters.cache_geometry),
    );
    let link = link_measurement("idle", &before, &after, span, &idle);
    cx.measure("idle link", link);
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

/// How long into an RX run the stop comes, and how long the leave may take:
/// the image's stop deadline is 2 s, its release and the answer follow it.
const STOP_AFTER: Duration = Duration::from_secs(2);
const LEAVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Leave while the host offers RX load the image receives and its socket
/// holds: the stop closes the socket and stops the IP service, every lent
/// buffer returns and the radio retires, so the station reports `Down`.
fn stop_under_load(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let device = device_address(cx)?;
    let host = host_route(device)?.source;
    let run = run_id();
    let endpoint = traffic::Endpoint::open(SocketAddrV4::new(host, 0), run)
        .map_err(|error| Failure::Incomplete(format!("the host socket: {error}")))?;
    let started = cx.call(
        Start {
            run,
            port: DEVICE_PORT,
            delay_ms: DELAY_MS,
            duration_ms: 10_000,
            drain_ms: DRAIN_MS,
            transmit: None,
            profile: None,
        },
        CALL,
    )?;
    check(
        started == Started::Running,
        format!("the image's load: {started:?}"),
    )?;
    let peer = SocketAddrV4::new(device, DEVICE_PORT);
    let offered_bps = 65_000_000;
    let (sent, left, took) = std::thread::scope(|scope| {
        let sender = scope.spawn(|| {
            traffic::send(
                &endpoint,
                peer,
                run,
                PAYLOAD,
                offered_bps,
                STOP_AFTER + LEAVE_TIMEOUT,
            )
        });
        std::thread::sleep(Duration::from_millis(u64::from(DELAY_MS)) + STOP_AFTER);
        let leaving = Instant::now();
        let left = cx.call(Leave, LEAVE_TIMEOUT);
        let took = leaving.elapsed();
        (sender.join(), left, took)
    });
    let left = left?;
    let status = status(cx)?;
    cx.measure(
        "stop under load",
        serde_json::json!({
            "offered_mbps": offered_bps / 1_000_000,
            "host_sent_datagrams": sent.as_ref().map_or(0, |sent| sent.datagrams),
            "leave_ms": took.as_millis() as u64,
            "left": format!("{left:?}"),
            "link": format!("{:?}", status.link),
        }),
    );
    check(
        left == Left::Down,
        format!("a leave under load ends the station normally: {left:?}"),
    )
}

fn levels(cx: &mut Cx<'_>, mode: Mode) -> Result<(), Failure> {
    let device = device_address(cx)?;
    let station = mac(status(cx)?.address);
    let host = host_route(device)?.source;
    for total in levels_mbps()? {
        level(cx, mode, device, &station, host, total)?;
    }
    Ok(())
}

/// One level: `total_mbps` offered, split over the mode's directions, for
/// the window, with RTT probes from the start to the end of the drain. The
/// access point's view of `station` is read before the run and after its
/// drain.
fn level(
    cx: &mut Cx<'_>,
    mode: Mode,
    device: Ipv4Addr,
    station: &str,
    host: Ipv4Addr,
    total_mbps: u64,
) -> Result<(), Failure> {
    let directions = if mode == Mode::RxTx { 2 } else { 1 };
    let each_bps = total_mbps
        .checked_mul(1_000_000)
        .map(|bps| bps / directions)
        .filter(|bps| u32::try_from(*bps).is_ok())
        .ok_or_else(|| Failure::Incomplete(format!("{total_mbps} Mbit/s is out of range")))?;
    let profile = profile::requested()?;
    let probing = probes()?;
    let run = run_id();
    let endpoint = traffic::Endpoint::open(SocketAddrV4::new(host, 0), run)
        .map_err(|error| Failure::Incomplete(format!("the host socket: {error}")))?;
    let host_port = endpoint
        .socket
        .local_addr()
        .map(|local| local.port())
        .unwrap_or(0);
    let before = link::read(cx, station);
    let since = Instant::now();
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
            profile,
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
        let probes = probing
            .then(|| scope.spawn(|| traffic::probe(&endpoint, peer, run, PROBE_PERIOD, &stop)));
        let sent = if mode == Mode::Tx {
            std::thread::sleep(window);
            traffic::Sent::default()
        } else {
            traffic::send(&endpoint, peer, run, PAYLOAD, each_bps, window)
        };
        stop.store(true, Ordering::Relaxed);
        let probes = probes.map_or(0, |probes| probes.join().unwrap_or(0));
        std::thread::sleep(Duration::from_millis(u64::from(DRAIN_MS)));
        (sent, probes)
    });
    let after = link::read(cx, station);
    let span = since.elapsed();
    let ended = collect(cx, run)?;
    let samples = match profile {
        Some(_) => Some(profile::collect(cx, run, CALL)?),
        None => None,
    };
    let mut received = endpoint.close();
    received.echoes.sent = probes;
    let name = format!("{} {total_mbps} Mbit/s", mode.name());
    let mut value = measurement(mode, total_mbps, &sent, &received, &ended);
    if let Some(object) = value.as_object_mut() {
        let link = link_measurement(&name, &before, &after, span, &ended);
        object.insert("link".to_owned(), link);
    }
    if let (Some(samples), Some(object)) = (samples, value.as_object_mut()) {
        let elf = profile::elf(IMAGE);
        let symbols = elf
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|elf| profile::symbolize(elf, &samples.samples));
        let sites = elf
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|elf| profile::copy_sites(elf));
        let elf = elf.unwrap_or_default();
        object.insert(
            "profile".to_owned(),
            profile::report(&samples, &elf, &symbols, &sites),
        );
    }
    cx.measure(name, value);
    Ok(())
}

/// One level's measurement. Throughput is over the measuring window: the
/// image's window for what it received and sent, the host's own sending
/// time for what the host sent, the image's window for what the host
/// received (the image sends only within it). CPU is the image's, at the
/// window's two ends, the parts' polls and the counters within it.
fn measurement(
    mode: Mode,
    total_mbps: u64,
    sent: &traffic::Sent,
    received: &traffic::Received,
    ended: &Ended,
) -> serde_json::Value {
    let report = &ended.report;
    let directions = if mode == Mode::RxTx { 2 } else { 1 };
    let offered_each = round(total_mbps as f64 / directions as f64);
    let window = window_us(report) as f64 / 1e6;
    let mut value = serde_json::json!({
        "offered_total_mbps": total_mbps,
        "payload_bytes": PAYLOAD,
        "window_s": round(window),
        "rtt": rtt(&received.echoes),
        "cpu": utilisation(&report.cpu_start, &report.cpu_end),
        "polls": polls(&ended.parts, window_us(report)),
        "counters": counters(&ended.counters, window_us(report)),
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
    fn the_width_is_ht20_or_ht40_and_nothing_else() {
        assert_eq!(parse_width("ht20").unwrap(), RouterChannelWidth::Ht20);
        assert_eq!(parse_width(" ht40 ").unwrap(), RouterChannelWidth::Ht40);
        assert!(parse_width("40").is_err());
    }

    #[test]
    fn probes_are_on_or_off_and_nothing_else() {
        assert!(parse_probes("on").unwrap());
        assert!(!parse_probes(" off ").unwrap());
        assert!(parse_probes("0").is_err());
    }

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
        let ended = Ended {
            report,
            parts: Vec::new(),
            counters: Counters::default(),
            link: Link::default(),
        };
        let value = measurement(Mode::Rx, 65, &sent, &traffic::Received::default(), &ended);
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
    fn the_counters_give_the_clock_the_instructions_per_cycle_and_per_stack_frame() {
        let value = counters(
            &Counters {
                cycles: 1_600_000_000,
                instructions: 480_000_000,
                bridge_received: 900,
                bridge_refused_pool: 400,
                stack_received: 500,
                ..Counters::default()
            },
            5_000_000,
        );
        assert_eq!(value["cycles_per_us"], 320.0);
        assert_eq!(value["instructions_per_cycle"], 0.3);
        assert_eq!(value["instructions_per_stack_frame"], 960_000.0);
        assert_eq!(value["bridge"]["refused_pool"], 400);
        let none = counters(&Counters::default(), 5_000_000);
        assert_eq!(none["rx"]["driver"]["available"], false);
        assert_eq!(none["rx"]["station"]["agreement_window"], 0);
        assert!(none["instructions_per_cycle"].is_null());
        assert!(none["instructions_per_stack_frame"].is_null());
    }

    #[test]
    fn a_bus_s_misses_are_rated_per_thousand_instructions_and_per_stack_frame() {
        let value = counters(
            &Counters {
                instructions: 4_000_000,
                stack_received: 4000,
                cache: oer_services_load::CacheCounts {
                    ibus0: InstructionBus {
                        hits: 98_000,
                        misses: 2_000,
                        conflicts: 5,
                        next_level_reads: 2_400,
                        overflowed: false,
                    },
                    dbus0: DataBus {
                        hits: 750,
                        misses: 250,
                        next_level_reads: 300,
                        next_level_writes: 100,
                        ..DataBus::default()
                    },
                    ..oer_services_load::CacheCounts::default()
                },
                ..Counters::default()
            },
            5_000_000,
        );
        let ibus0 = &value["cache"]["ibus0"];
        assert_eq!(ibus0["misses"], 2_000);
        assert_eq!(ibus0["conflicts"], 5);
        assert_eq!(ibus0["miss_percent"], 2.0);
        assert_eq!(ibus0["misses_per_1000_instructions"], 0.5);
        assert_eq!(ibus0["misses_per_stack_frame"], 0.5);
        assert_eq!(ibus0["next_level_reads_per_stack_frame"], 0.6);
        let dbus0 = &value["cache"]["dbus0"];
        assert_eq!(dbus0["miss_percent"], 25.0);
        assert_eq!(dbus0["next_level_writes_per_stack_frame"], 0.025);
        assert_eq!(dbus0["next_level_writes_per_1000_instructions"], 0.025);
        assert!(
            value["cache"]["ibus1"]["miss_percent"].is_null(),
            "no access"
        );
        assert_eq!(value["cache"]["ibus1"]["misses_per_stack_frame"], 0.0);
    }

    #[test]
    fn an_overflowed_bus_or_a_missing_denominator_gives_no_rate() {
        let value = counters(
            &Counters {
                instructions: 1000,
                cache: oer_services_load::CacheCounts {
                    dbus1: DataBus {
                        hits: 10,
                        misses: 10,
                        overflowed: true,
                        ..DataBus::default()
                    },
                    ..oer_services_load::CacheCounts::default()
                },
                ..Counters::default()
            },
            5_000_000,
        );
        let dbus1 = &value["cache"]["dbus1"];
        assert_eq!(dbus1["overflowed"], true);
        assert_eq!(dbus1["misses"], 10, "the raw count stays");
        assert!(dbus1["miss_percent"].is_null());
        assert!(dbus1["misses_per_1000_instructions"].is_null());
        let ibus0 = &value["cache"]["ibus0"];
        assert_eq!(ibus0["misses_per_1000_instructions"], 0.0);
        assert!(ibus0["misses_per_stack_frame"].is_null(), "no stack frame");
    }

    #[test]
    fn the_geometry_names_each_cache_s_size_line_and_ways() {
        let geometry = CacheGeometry {
            l1_icache: CacheLevel {
                size_bytes: Some(32 * 1024),
                line_bytes: Some(64),
                ways: Some(2),
            },
            l2_bypassed: true,
            ..CacheGeometry::default()
        };
        let value = cache_geometry(&geometry);
        assert_eq!(value["l1_icache"]["size_bytes"], 32 * 1024);
        assert_eq!(value["l1_icache"]["ways"], 2);
        assert!(value["l2"]["size_bytes"].is_null());
        assert_eq!(value["l2_bypassed"], true);
    }

    #[test]
    fn a_part_s_polls_are_rated_over_the_window() {
        let part = oer_services_load::PartWindow {
            name: "ip".try_into().unwrap(),
            polls: 2000,
            poll_us: 1_500_000,
            ended_poll_us: 1_500_000,
            longest_poll_us: Some(900),
        };
        let idle = oer_services_load::PartWindow {
            name: "console".try_into().unwrap(),
            ..Default::default()
        };
        let value = polls(&[part, idle], 5_000_000);
        assert_eq!(value[0]["part"], "ip");
        assert_eq!(value[0]["polls_per_s"], 400.0);
        assert_eq!(value[0]["poll_time_percent"], 30.0);
        assert_eq!(value[0]["mean_poll_us"], 750.0);
        assert_eq!(value[0]["longest_poll_us"], 900);
        assert_eq!(value[1]["mean_poll_us"], 0.0);
        assert!(value[1]["longest_poll_us"].is_null());
    }

    #[test]
    fn a_poll_open_at_the_window_s_end_counts_in_the_share_not_the_mean() {
        let part = oer_services_load::PartWindow {
            name: "station".try_into().unwrap(),
            polls: 1,
            poll_us: 10 + 979,
            ended_poll_us: 10,
            longest_poll_us: Some(10),
        };
        let value = polls(&[part], 1_000);
        assert_eq!(value[0]["poll_time_percent"], 98.9);
        assert_eq!(value[0]["mean_poll_us"], 10.0);
        assert_eq!(value[0]["longest_poll_us"], 10);
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
