//! The HIL Wi-Fi station load scenario. The image's station joins the lab
//! router's isolated network, HT 40 MHz wide, which forwards UDP between it
//! and this host (wired to the router: one hop over the air). The host sends and receives
//! the load; the image counts its end and reports its CPU over the HIL link.
//! RX and TX are named from the image's side. Every level of every mode is
//! measured and reported (`target/hil/results/`): the measurements never
//! decide a case, only a path or an exchange that does not work does.
//! Each window also records what the link ran ([`link`]): the access
//! point's view of the station and the image's rates each way, and what
//! the image's periodic PHY tracking did ([`tracking`]), and where its
//! receive path last moved by boundary, with its radio owner's snapshot at
//! the window's end ([`stall`]).
//! With `OER_WIFI_LOAD_PROFILE_US` each level also samples where the
//! image's hart spends its window ([`profile`]), and with
//! `OER_WIFI_LOAD_DIAGNOSTICS=on` it records the network core's switchable
//! diagnostics over its window ([`diagnostics`]).
#![forbid(unsafe_code)]

mod addba;
mod diagnostics;
mod link;
mod ping;
mod profile;
mod profile_turns;
mod stall;
mod tracking;
mod traffic;
mod turns;
mod vendor;

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    process::ExitCode,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use oer_hil_services_load::{
    Addba, AddbaCollected, Begin, Begun, CacheGeometry, CacheLevel, Collect, CollectAddba,
    CollectCounters, CollectDiagnostics, CollectHart1, CollectLink, CollectMac, CollectNetwork,
    CollectParts, CollectPositions, CollectProfile, CollectStall, CollectStallMac, CollectTracking,
    Collected, Counters, CountersCollected, DataBus, Diagnose, DiagnosticsCollected,
    Hart1Collected, Hart1Window, InstructionBus, Link, LinkCollected, Load, MAC_COUNT_NAMES,
    MAX_PARTS, Mac, MacCollected, NetworkCollected, NetworkTurns, PROFILE_PAGES, PartWindow,
    PartsCollected, Positions, PositionsCollected, ProfileCollected, Report, Stall, StallCollected,
    StallMac, StallMacCollected, Start, Started, TUNE_KNOBS, TUNE_UNSET, Tracking,
    TrackingCollected, Transmit,
};
use oer_hil_services_wifi_station::{
    GetConnectTimes, GetScanVisit, GetStatus, Join, Joined, Leave, Left, ScanVisit,
    ScanVisitAnswer, Status,
};
use oer_protocols_hil_agent::base::Service as _;
use oer_tools_hil_harness::{
    BuildChoice, Case, Cx, Failure, RouterChannelWidth, RouterOperation, Scenario, check,
};

/// The image the scenario runs: the station on the network core.
const IMAGE: &str = "hil-wifi-station";

const SCENARIO: Scenario = Scenario::new(
    "wifi-load",
    IMAGE,
    &[
        oer_hil_services_wifi_station::Request::KEYS,
        oer_hil_services_load::Request::KEYS,
    ],
)
.using_router()
.using_air_exclusively();

const CASES: [Case; 9] = [
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
    Case {
        name: "ping",
        run: ping_idle,
    },
    Case {
        name: "combined",
        run: combined,
    },
    Case {
        name: "overload",
        run: overload,
    },
    Case {
        name: "sparse",
        run: sparse,
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
const PAYLOAD: usize = 1472;
/// Total offered load of each level, Mbit/s; a mode with two directions
/// offers half each way. `OER_WIFI_LOAD_MBPS` (comma-separated) replaces
/// them, e.g. to offer more than these levels; level 0 is no pacing in
/// every direction ([`traffic::Rate::Unpaced`]): each sender sends as fast
/// as its socket takes the datagrams, which measures the ceiling. A paced
/// level above the ceiling stays behind the sender's own sleeps.
const LEVELS_MBPS: [u64; 4] = [10, 30, 50, 65];
const LEVELS_ENV: &str = "OER_WIFI_LOAD_MBPS";
/// Each level's measuring window, the time before it opens and the drain
/// after it (the image's run, `oer_hil_services_load::Start`).
const DURATION_MS: u32 = 5000;
/// Replaces each level's window, in seconds.
const SECONDS_ENV: &str = "OER_WIFI_LOAD_SECONDS";
const DELAY_MS: u32 = 300;
const DRAIN_MS: u32 = 500;
/// RTT probes during a level. `OER_WIFI_LOAD_PROBES=off` sends none (`on`
/// is the default), to measure a level without the probes' datagrams.
const PROBE_PERIOD: Duration = Duration::from_millis(50);
const PROBES_ENV: &str = "OER_WIFI_LOAD_PROBES";
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);
const DHCP_TIMEOUT: Duration = Duration::from_secs(30);
const REPORT_TIMEOUT: Duration = Duration::from_secs(2);
const CALL: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    match vendor::requested() {
        Ok(Some(variant)) => return vendor::main(variant),
        Ok(None) => {}
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    }
    oer_tools_hil_harness::main(&SCENARIO, &CASES)
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

fn stand() -> Result<oer_tools_lab_model::Stand, Failure> {
    oer_tools_lab_model::path()
        .and_then(|path| oer_tools_lab_model::Stand::load(&path))
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

/// The image's address once it has one: the device's own lease, read from
/// its status.
fn device_address(cx: &mut Cx<'_>) -> Result<Ipv4Addr, Failure> {
    status(cx)?
        .ipv4
        .map(|ipv4| Ipv4Addr::from(ipv4.address))
        .ok_or_else(|| Failure::Incomplete("the station has no address: run `path` first".into()))
}

/// The report of `run` and its parts' window, once the image ended it.
/// What the image kept of one run.
/// What a run kept. The report, the boot hart's counters and hart 1's come
/// from every build; the rest from the trace's observers, `None` from the
/// measuring build, which answers [`oer_hil_services_load::LoadError::NotBuilt`] for them.
struct Ended {
    report: Report,
    counters: Counters,
    hart1: Hart1Window,
    traced: Option<Traced>,
}

/// What only the trace build reads.
struct Traced {
    parts: Vec<PartWindow>,
    link: Link,
    mac: Mac,
    positions: Positions,
    tracking: Tracking,
    stall: Stall,
    stall_mac: StallMac,
    network: NetworkTurns,
    addba: Addba,
    turn_profile: profile_turns::Profile,
}

/// The instrumentation class of the trace, as the bundle names it.
const TRACE: &str = "trace";

/// Whether the image the harness built has the trace: without it, the
/// measuring build, a collection of the trace's counts is refused as
/// [`oer_hil_services_load::LoadError::NotBuilt`], and only then.
fn traced(built: &BuildChoice) -> bool {
    built.instrumentation.iter().any(|class| class == TRACE)
}

fn collect(cx: &mut Cx<'_>, run: u32) -> Result<Ended, Failure> {
    let report = collect_report(cx, run)?;
    collect_ended(cx, run, report)
}

fn collect_report(cx: &mut Cx<'_>, run: u32) -> Result<Report, Failure> {
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
    Ok(report)
}

fn collect_ended(cx: &mut Cx<'_>, run: u32, report: Report) -> Result<Ended, Failure> {
    let counters = match cx.call(CollectCounters { run }, CALL)? {
        CountersCollected::Report(counters) => counters,
        other => return Err(Failure::Fail(format!("the image's counters: {other:?}"))),
    };
    let hart1 = match cx.call(CollectHart1 { run }, CALL)? {
        Hart1Collected::Report(hart1) => hart1,
        other => return Err(Failure::Fail(format!("the image's hart 1: {other:?}"))),
    };
    received_readable(&report)?;
    let traced = if traced(cx.built()) {
        Some(collect_traced(cx, run)?)
    } else {
        // The measuring build refuses what it did not build: one such
        // refusal confirms the image is the one the harness built.
        match cx.call(CollectLink { run }, CALL)? {
            LinkCollected::Refused(oer_hil_services_load::LoadError::NotBuilt) => None,
            other => {
                return Err(Failure::Fail(format!(
                    "the measuring image's link: {other:?}, not NotBuilt"
                )));
            }
        }
    };
    Ok(Ended {
        report,
        counters,
        hart1,
        traced,
    })
}

/// What the trace build kept of run `run`.
fn collect_traced(cx: &mut Cx<'_>, run: u32) -> Result<Traced, Failure> {
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
    let link = match cx.call(CollectLink { run }, CALL)? {
        LinkCollected::Report(link) => link,
        other => return Err(Failure::Fail(format!("the image's link: {other:?}"))),
    };
    let mac = match cx.call(CollectMac { run }, CALL)? {
        MacCollected::Report(mac) => mac,
        other => return Err(Failure::Fail(format!("the image's MAC: {other:?}"))),
    };
    let positions = match cx.call(CollectPositions { run }, CALL)? {
        PositionsCollected::Report(positions) => positions,
        other => return Err(Failure::Fail(format!("the image's positions: {other:?}"))),
    };
    let tracking = match cx.call(CollectTracking { run }, CALL)? {
        TrackingCollected::Report(tracking) => tracking,
        other => return Err(Failure::Fail(format!("the image's tracking: {other:?}"))),
    };
    let stall = match cx.call(CollectStall { run }, CALL)? {
        StallCollected::Report(stall) => stall,
        other => return Err(Failure::Fail(format!("the image's stall: {other:?}"))),
    };
    let stall_mac = match cx.call(CollectStallMac { run }, CALL)? {
        StallMacCollected::Report(stall_mac) => stall_mac,
        other => return Err(Failure::Fail(format!("the image's stall MAC: {other:?}"))),
    };
    let network = match cx.call(CollectNetwork { run }, CALL)? {
        NetworkCollected::Report(network) => network,
        other => {
            return Err(Failure::Fail(format!(
                "the image's network turns: {other:?}"
            )));
        }
    };
    let mut turn_profile = profile_turns::Profile::default();
    for page in 0..PROFILE_PAGES {
        match cx.call(CollectProfile { run, page }, CALL)? {
            ProfileCollected::Moments(moments) => turn_profile.moments = moments,
            ProfileCollected::Cells(cells) => turn_profile.cells = cells,
            ProfileCollected::Spread(spread) => turn_profile.spread = spread,
            other => {
                return Err(Failure::Fail(format!(
                    "the image's turn profile, page {page}: {other:?}"
                )));
            }
        }
    }
    match cx.call(CollectAddba { run }, CALL)? {
        AddbaCollected::Report(addba) => Ok(Traced {
            parts,
            link,
            mac,
            positions,
            tracking,
            stall,
            stall_mac,
            network,
            addba,
            turn_profile,
        }),
        other => Err(Failure::Fail(format!(
            "the image's ADDBA requests: {other:?}"
        ))),
    }
}

/// The reason a run fails whose image met lent datagrams it could not read.
const UNREADABLE: &str = "unreadable lent datagrams";

/// A run's report holds only datagrams the image could read: a lent buffer
/// that no longer held what the station showed breaks the device's
/// contract, and the case fails with [`UNREADABLE`], whatever else it
/// measured.
fn received_readable(report: &Report) -> Result<(), Failure> {
    match report.received.unreadable {
        0 => Ok(()),
        count => Err(Failure::Fail(format!(
            "{UNREADABLE}: {count}, whose buffer no longer held what the station showed"
        ))),
    }
}

/// The MAC's counts that moved over a window, by name, and its frequency
/// offset.
fn mac_counts(mac: &Mac) -> serde_json::Value {
    let moved: serde_json::Map<_, _> = MAC_COUNT_NAMES
        .iter()
        .enumerate()
        .map(|(index, (name, _))| ((*name).to_owned(), mac.count(index)))
        .filter(|(_, count)| *count != 0)
        .map(|(name, count)| (name, count.into()))
        .collect();
    serde_json::json!({ "counts": moved, "frequency_offset": mac.frequency_offset })
}

/// The aggregates a BlockAck answered at their first transmission, by
/// position on air.
fn positions_counts(positions: &Positions) -> serde_json::Value {
    serde_json::json!({
        "sent": positions.sent,
        "holes": positions.holes,
        "tail_runs": positions.tail_runs,
        "interior": positions.interior,
    })
}

/// What the image's periodic PHY tracking did over a window; its one-line
/// summary is printed as `name`'s.
fn tracking_measurement(name: &str, traced: &Traced) -> serde_json::Value {
    println!("  tracking {name}: {}", tracking::summary(&traced.tracking));
    tracking::window(&traced.tracking)
}

/// The ADDBA Requests the station took since boot, at the window's end; its
/// one-line summary is printed as `name`'s.
fn addba_measurement(name: &str, traced: &Traced) -> serde_json::Value {
    println!("  addba {name}: {}", addba::summary(&traced.addba));
    addba::window(&traced.addba)
}

/// Where the receive path last moved and the radio owner's snapshot at the
/// window's end; its one-line summary is printed as `name`'s.
fn stall_measurement(name: &str, report: &Report, traced: &Traced) -> serde_json::Value {
    let (stall, mac) = (&traced.stall, &traced.stall_mac);
    println!("  stall {name}: {}", stall::summary(report, stall, mac));
    stall::window(report, stall, mac)
}

/// What the network core's turns did over a window; its one-line summary
/// is printed as `name`'s.
fn turns_measurement(name: &str, traced: &Traced) -> serde_json::Value {
    println!(
        "  network turns {name}: {}",
        turns::summary(&traced.network)
    );
    turns::window(&traced.network)
}

/// The network core's turn profile over a window; its one-line summary is
/// printed as `name`'s.
fn profile_measurement(name: &str, traced: &Traced) -> serde_json::Value {
    println!(
        "  turn profile {name}: {}",
        profile_turns::summary(&traced.turn_profile)
    );
    profile_turns::window(&traced.turn_profile)
}

/// What the link ran over a window: the access point's view between
/// `before` and `after`, `span` apart, and the image's counts in `ended`
/// where its build keeps them (`null` from the measuring build); its
/// one-line summary is printed as `name`'s.
fn link_measurement(
    name: &str,
    before: &link::Reading,
    after: &link::Reading,
    span: Duration,
    ended: &Ended,
) -> serde_json::Value {
    let router = link::router_window(before, after, span);
    match &ended.traced {
        Some(traced) => {
            let device = link::device(&traced.link);
            println!("  link {name}: {}", link::summary(&router, &device));
            serde_json::json!({ "router": router, "device": device })
        }
        None => serde_json::json!({ "router": router, "device": null }),
    }
}

/// The highest total a level may offer: far above any 2.4 GHz link, and
/// each direction's rate stays a `u32` of bit/s.
const MAX_LEVEL_MBPS: u64 = 1000;

/// The channel width the router's profile asks for: HT40, or the
/// `OER_WIFI_LOAD_WIDTH` it names (`ht20`, `ht40`, or `he20`: the router's
/// HE20 station profile, and the station's join asks for HE20).
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
        "he20" => Ok(RouterChannelWidth::He20),
        other => Err(Failure::Incomplete(format!(
            "{WIDTH_ENV}=`{other}` is none of `ht20`, `ht40` and `he20`"
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

/// A level's window in milliseconds.
fn window_ms() -> Result<u32, Failure> {
    match std::env::var(SECONDS_ENV) {
        Ok(text) => parse_seconds(&text).map_err(Failure::Incomplete),
        Err(_) => Ok(DURATION_MS),
    }
}

fn parse_seconds(text: &str) -> Result<u32, String> {
    text.parse::<u32>()
        .ok()
        .filter(|seconds| (1..=60).contains(seconds))
        .map(|seconds| seconds * 1000)
        .ok_or_else(|| format!("{SECONDS_ENV}: `{text}` is not 1 to 60 seconds"))
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

/// The levels to offer: [`LEVELS_MBPS`], or `OER_WIFI_LOAD_MBPS`, each 0 (no
/// pacing, as fast as the socket takes the datagrams) to [`MAX_LEVEL_MBPS`].
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
                .filter(|level| *level <= MAX_LEVEL_MBPS)
        })
        .collect::<Option<Vec<_>>>()
        .filter(|levels| !levels.is_empty())
        .ok_or_else(|| {
            Failure::Incomplete(format!(
                "{LEVELS_ENV}=`{text}` is no list of 0 to {MAX_LEVEL_MBPS} Mbit/s"
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
            let wakes_before = before
                .harts
                .iter()
                .find(|earlier| earlier.hart == hart.hart)
                .map_or(0, |earlier| earlier.wakes);
            serde_json::json!({
                "hart": hart.hart,
                "busy_percent": round(100.0 * (1.0 - idle / window)),
                "wakes_per_s": round(
                    f64::from(hart.wakes.wrapping_sub(wakes_before)) * 1e6 / window,
                ),
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
            let rounds_before = before
                .executors
                .iter()
                .find(|earlier| earlier.executor == executor.executor)
                .map_or(0, |earlier| earlier.rounds);
            serde_json::json!({
                "executor": executor.executor,
                "hart": executor.hart,
                "level": executor.level,
                "work_percent": round(
                    100.0 * executor.work_us.saturating_sub(work_before) as f64 / window,
                ),
                "rounds_per_s": round(
                    f64::from(executor.rounds.wrapping_sub(rounds_before)) * 1e6 / window,
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

/// Hart 1's counters over the window, read on hart 1 at its very ends:
/// both ends, their differences, and those per microsecond (MHz) and per
/// cycle. A counter that did not advance from `start` to `end` has no
/// difference and `advanced` false: a broken reading, not a zero.
fn hart1(hart1: &Hart1Window, window_us: u64) -> serde_json::Value {
    let window = window_us.max(1) as f64;
    let Hart1Window { start, end } = hart1;
    let cycles = end.cycles.checked_sub(start.cycles);
    let instructions = end.instructions.checked_sub(start.instructions);
    serde_json::json!({
        "start": start,
        "end": end,
        "advanced": cycles.is_some_and(|count| count != 0)
            && instructions.is_some_and(|count| count != 0),
        "cycles": cycles,
        "instructions": instructions,
        "cycles_per_us": cycles.map(|count| round(count as f64 / window)),
        "instructions_per_cycle": cycles
            .zip(instructions)
            .filter(|(cycles, _)| *cycles != 0)
            .map(|(cycles, instructions)| fine(instructions as f64 / cycles as f64)),
    })
}

/// The window's counters: the image's measured hart's cycles and retired
/// instructions with their rate per microsecond (MHz) and per cycle, what
/// the datagrams the
/// network delivered with the hart's instructions over them, and the L1 cache's
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
        "stack_received": counters.stack_received,
        "instructions_per_stack_frame": (counters.stack_received != 0)
            .then(|| round(instructions / f64::from(counters.stack_received))),
        "cache": cache(counters),
        "rx": counters.rx,
        "tx": tx_counts(counters),
    })
}

/// The transmit path's counts: the station's settled aggregates with their
/// mean subframes (null without one) beside the image's counts; null from
/// the measuring build, which keeps none.
fn tx_counts(counters: &Counters) -> serde_json::Value {
    let Some(tx) = &counters.tx else {
        return serde_json::Value::Null;
    };
    let ampdu = &tx.ampdu;
    let mut value = serde_json::json!(tx);
    value["ampdu"]["average_subframes"] = serde_json::json!(
        (ampdu.aggregates != 0)
            .then(|| round(f64::from(ampdu.subframes) / f64::from(ampdu.aggregates)))
    );
    value
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
/// hart's instructions and per datagram the network delivered. An overflowed
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
    let width = width()?;
    cx.router(RouterOperation::ApplyStationProfile {
        ssid: ssid.clone(),
        passphrase: passphrase.clone(),
        channel,
        width,
        forward_host: Some(host),
    })?;
    let join = Join {
        ssid: heapless::String::try_from(ssid.as_str())
            .map_err(|_| Failure::Incomplete("the generated SSID is too long".into()))?,
        passphrase: heapless::String::try_from(passphrase.as_str())
            .map_err(|_| Failure::Incomplete("the generated passphrase is too long".into()))?,
        channel,
        he20: width == RouterChannelWidth::He20,
    };
    let joined = cx.call(join, JOIN_TIMEOUT)?;
    // The connect's timeline is the trace's: the measuring build keeps none.
    if traced(cx.built()) {
        println!("connect: {}", cx.call(GetConnectTimes, REPORT_TIMEOUT)?);
        println!("connect: {}", ScanVisit::HEADER);
        let mut index = 0;
        while let ScanVisitAnswer(Some(visit)) = cx.call(GetScanVisit { index }, REPORT_TIMEOUT)? {
            println!("connect: {visit}");
            index += 1;
        }
    }
    let (bssid, linked) = match joined {
        Joined::Linked {
            bssid,
            channel,
            phy,
        } => {
            println!("connect: linked phy={phy:?}");
            (bssid, channel)
        }
        other => {
            return Err(Failure::Fail(format!(
                "the station did not join: {other:?}"
            )));
        }
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
            synchronised: false,
            duration_ms: 1000,
            drain_ms: DRAIN_MS,
            transmit: Some(Transmit {
                peer: source.octets(),
                peer_port: host_port,
                payload: PAYLOAD as u16,
                rate_bps: 100_000,
                burst: 1,
            }),
            profile: None,
            tune: [TUNE_UNSET; TUNE_KNOBS],
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
            synchronised: false,
            duration_ms: DURATION_MS,
            drain_ms: 0,
            transmit: None,
            profile: None,
            tune: [TUNE_UNSET; TUNE_KNOBS],
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
        if let Some(traced) = &idle.traced {
            object.insert("parts".to_owned(), polls(&traced.parts, window));
        }
        object.insert("counters".to_owned(), counters(&idle.counters, window));
        object.insert("hart1".to_owned(), hart1(&idle.hart1, window));
        object.insert("build".to_owned(), build(cx.built()));
    }
    cx.measure("idle cpu", cpu);
    cx.measure(
        "cache geometry",
        cache_geometry(&idle.counters.cache_geometry),
    );
    let link = link_measurement("idle", &before, &after, span, &idle);
    cx.measure("idle link", link);
    if let Some(traced) = &idle.traced {
        let tracking = tracking_measurement("idle", traced);
        cx.measure("idle tracking", tracking);
        cx.measure("idle addba", addba_measurement("idle", traced));
    }
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
    fn directions(self) -> u64 {
        if self == Self::RxTx { 2 } else { 1 }
    }

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

/// Each way of the `combined` case: 130 Mbit/s in all, near the air's
/// ceiling (the archive's network comparison).
const COMBINED_MBPS: u64 = 65;
/// Each way of the `overload` case: more than the air holds either way
/// (the archive's network comparison).
const OVERLOAD_MBPS: u64 = 130;
/// The requests of the `ping` case: some 13.6 s at [`ping::PERIOD`].
const PING_COUNT: u32 = 120;
/// The longest median round trip of a ping over an unloaded link, the
/// owner's limit: host, wire, router, one hop over the air and back.
const PING_MEDIAN_LIMIT_US: u64 = 3000;
/// The replies a ping's median needs to decide a case: fewer leave it
/// incomplete, as a median of a few depends on where they fell.
const PING_REPLIES_LEAST: usize = 30;

/// One sparse traffic shape, the same each way.
#[derive(Clone, Copy)]
struct Sparse {
    datagrams: u16,
    every_ms: u32,
    payload: usize,
}

impl Sparse {
    fn name(self) -> String {
        format!(
            "sparse {} of {} bytes every {} ms",
            self.datagrams, self.payload, self.every_ms
        )
    }

    /// The bits per second of UDP payload the shape offers one way.
    fn rate_bps(self) -> u64 {
        u64::from(self.datagrams) * self.payload as u64 * 8 * 1000 / u64::from(self.every_ms)
    }

    /// The shape's paced rate.
    fn rate(self) -> Result<traffic::Rate, Failure> {
        traffic::Rate::paced(self.rate_bps())
            .ok_or_else(|| Failure::Incomplete(format!("{} has no paced rate", self.name())))
    }
}

/// Isolated frames from an idle link, the archive's sparse peer of two
/// datagrams, lone frames about as far apart as the receive ring's longest
/// wait, and bursts that end in silence; small frames take the ring's
/// shorter wait.
const SPARSE: [Sparse; 6] = [
    Sparse {
        datagrams: 1,
        every_ms: 100,
        payload: PAYLOAD,
    },
    Sparse {
        datagrams: 2,
        every_ms: 100,
        payload: PAYLOAD,
    },
    Sparse {
        datagrams: 1,
        every_ms: 1,
        payload: PAYLOAD,
    },
    Sparse {
        datagrams: 8,
        every_ms: 20,
        payload: PAYLOAD,
    },
    Sparse {
        datagrams: 32,
        every_ms: 50,
        payload: PAYLOAD,
    },
    Sparse {
        datagrams: 8,
        every_ms: 20,
        payload: 200,
    },
];

/// Over an idle link: the median round trip decides the case.
fn ping_idle(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let device = device_address(cx)?;
    let echoes = ping::echo(device, PING_COUNT).map_err(Failure::Incomplete)?;
    cx.measure("ping idle", rtt(&echoes));
    ping_within_limit("idle", &echoes)
}

/// Whether the ping's median is within [`PING_MEDIAN_LIMIT_US`]; with
/// fewer than [`PING_REPLIES_LEAST`] replies the case is incomplete.
fn ping_within_limit(name: &str, echoes: &traffic::Echoes) -> Result<(), Failure> {
    if echoes.rtt_us.len() < PING_REPLIES_LEAST {
        return Err(Failure::Incomplete(format!(
            "ping {name}: {} replies of {} requests, fewer than the {PING_REPLIES_LEAST} a median needs",
            echoes.rtt_us.len(),
            echoes.sent
        )));
    }
    let median = ping::median_us(echoes);
    check(
        median.is_some_and(|median| median <= PING_MEDIAN_LIMIT_US),
        format!(
            "ping {name}: median {median:?} us of {} replies, the limit {PING_MEDIAN_LIMIT_US} us",
            echoes.rtt_us.len()
        ),
    )
}

fn combined(cx: &mut Cx<'_>) -> Result<(), Failure> {
    both_ways(cx, "combined", COMBINED_MBPS)
}

fn overload(cx: &mut Cx<'_>) -> Result<(), Failure> {
    both_ways(cx, "overload", OVERLOAD_MBPS)
}

/// One level of `each_mbps` each way, a ping beside it.
fn both_ways(cx: &mut Cx<'_>, name: &str, each_mbps: u64) -> Result<(), Failure> {
    let path = Path::of(cx)?;
    let offer = Offer {
        name: format!("{name} {each_mbps} + {each_mbps} Mbit/s"),
        ping: true,
        ..Offer::level(Mode::RxTx, each_mbps * 2, probes()?)?
    };
    tuned(cx, &path, offer).map(|_| ())
}

/// Every shape of [`SPARSE`] both ways at once, a ping beside it and no
/// RTT probes, which would be traffic of their own. A shape's median ping
/// decides the case: a sparse link stays as quick as an idle one.
fn sparse(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let path = Path::of(cx)?;
    let mut over = Ok(());
    for shape in SPARSE {
        let name = shape.name();
        let offer = Offer {
            name,
            mode: Mode::RxTx,
            each: shape.rate()?,
            payload: shape.payload,
            burst: shape.datagrams,
            probing: false,
            ping: true,
            tune: [TUNE_UNSET; TUNE_KNOBS],
        };
        for (name, echoes) in tuned(cx, &path, offer)? {
            over = over.and(ping_within_limit(&name, &echoes));
        }
    }
    over
}

/// How long into an RX run the stop comes, and how long the leave may take:
/// the image's stop deadline is 2 s, its release and the answer follow it.
const STOP_AFTER: Duration = Duration::from_secs(2);
const LEAVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Leave while the host offers RX load the image receives and its socket
/// holds: the stop closes the socket and stops the network, every lent
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
            synchronised: false,
            duration_ms: 10_000,
            drain_ms: DRAIN_MS,
            transmit: None,
            profile: None,
            tune: [TUNE_UNSET; TUNE_KNOBS],
        },
        CALL,
    )?;
    check(
        started == Started::Running,
        format!("the image's load: {started:?}"),
    )?;
    let peer = SocketAddrV4::new(device, DEVICE_PORT);
    let offered = traffic::Rate::paced(65_000_000)
        .ok_or_else(|| Failure::Incomplete("the stop case's rate".to_owned()))?;
    let (sent, left, took) = std::thread::scope(|scope| {
        let sender = scope.spawn(|| {
            traffic::send(
                &endpoint,
                peer,
                run,
                PAYLOAD,
                offered,
                1,
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
            "offered_mbps": offered.mbps(),
            "host_sent_datagrams": sent.as_ref().map_or(0, |sent| sent.datagrams),
            "leave_ms": took.as_millis() as u64,
            "left": format!("{left:?}"),
            "link": format!("{:?}", status.link),
        }),
    );
    check(
        left == Left::Down,
        match left {
            Left::DownNetworkLost => {
                "a leave under load stopped the radio, but the network's stop participant was lost"
                    .into()
            }
            left => format!("a leave under load ends the station normally: {left:?}"),
        },
    )
}

fn levels(cx: &mut Cx<'_>, mode: Mode) -> Result<(), Failure> {
    let path = Path::of(cx)?;
    let probing = probes()?;
    for total in levels_mbps()? {
        tuned(cx, &path, Offer::level(mode, total, probing)?)?;
    }
    Ok(())
}

/// The knobs of a trace image a run may set (`oer_portable_trace::tune`),
/// in order.
const TUNE_NAMES: [&str; 2] = ["return-quiet", "recycled-burst"];
const TUNES_ENV: &str = "OER_WIFI_LOAD_TUNES";

/// `OER_WIFI_LOAD_TUNES`: each level once per `;`-separated set of
/// `name=value`, an empty set keeping the defaults. Values go to the image
/// at the run's start: comparing them takes one build and one boot.
fn tunes() -> Result<Vec<[u32; TUNE_KNOBS]>, Failure> {
    match std::env::var(TUNES_ENV) {
        Err(_) => Ok(vec![[TUNE_UNSET; TUNE_KNOBS]]),
        Ok(text) => parse_tunes(&text),
    }
}

fn parse_tunes(text: &str) -> Result<Vec<[u32; TUNE_KNOBS]>, Failure> {
    let refused = |what: &str| Failure::Incomplete(format!("{TUNES_ENV}: `{what}`"));
    text.split(';')
        .map(|set| {
            let mut tune = [TUNE_UNSET; TUNE_KNOBS];
            for item in set
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
            {
                let (name, value) = item.split_once('=').ok_or_else(|| refused(item))?;
                let place = TUNE_NAMES
                    .iter()
                    .position(|known| *known == name)
                    .and_then(|place| tune.get_mut(place))
                    .ok_or_else(|| refused(item))?;
                *place = value.parse().map_err(|_| refused(item))?;
            }
            Ok(tune)
        })
        .collect()
}

/// `tune`'s `name=value` list, the level's name's tail.
fn tune_name(tune: &[u32; TUNE_KNOBS]) -> String {
    TUNE_NAMES
        .iter()
        .zip(tune)
        .filter(|(_, value)| **value != TUNE_UNSET)
        .map(|(name, value)| format!(" {name}={value}"))
        .collect()
}

/// `offer` once per set of [`tunes`]: each level's name and its ping.
fn tuned(
    cx: &mut Cx<'_>,
    path: &Path,
    offer: Offer,
) -> Result<Vec<(String, traffic::Echoes)>, Failure> {
    let mut ends = Vec::new();
    for tune in tunes()? {
        let tuned = Offer {
            name: format!("{}{}", offer.name, tune_name(&tune)),
            tune,
            ..offer
        };
        let echoes = level(cx, path, &tuned)?;
        ends.push((tuned.name, echoes));
    }
    Ok(ends)
}

/// Who a level runs between.
struct Path {
    device: Ipv4Addr,
    station: String,
    host: Ipv4Addr,
}

impl Path {
    fn of(cx: &mut Cx<'_>) -> Result<Self, Failure> {
        let device = device_address(cx)?;
        Ok(Self {
            device,
            station: mac(status(cx)?.address),
            host: host_route(device)?.source,
        })
    }
}

/// What a level offers.
struct Offer {
    name: String,
    mode: Mode,
    /// Each direction's rate of UDP payload.
    each: traffic::Rate,
    payload: usize,
    /// The datagrams due together, each way.
    burst: u16,
    probing: bool,
    /// Whether a `ping` runs through the window beside the load.
    ping: bool,
    /// The run's values of the image's constants.
    tune: [u32; TUNE_KNOBS],
}

impl Offer {
    /// `total_mbps` of full datagrams, split over `mode`'s directions; level
    /// 0 is no pacing, in every direction the mode has.
    fn level(mode: Mode, total_mbps: u64, probing: bool) -> Result<Self, Failure> {
        let (each, name) = match total_mbps {
            0 => (traffic::Rate::Unpaced, format!("{} unpaced", mode.name())),
            _ => (
                total_mbps
                    .checked_mul(1_000_000)
                    .map(|bps| bps / mode.directions())
                    .and_then(traffic::Rate::paced)
                    .ok_or_else(|| {
                        Failure::Incomplete(format!("{total_mbps} Mbit/s is out of range"))
                    })?,
                format!("{} {total_mbps} Mbit/s", mode.name()),
            ),
        };
        Ok(Self {
            name,
            mode,
            each,
            payload: PAYLOAD,
            burst: 1,
            probing,
            ping: false,
            tune: [TUNE_UNSET; TUNE_KNOBS],
        })
    }
}

/// What the image sends in `offer`'s run: nothing for RX, otherwise the load
/// at the offer's rate, where the wire's rate 0 is the unpaced rate.
fn device_transmit(offer: &Offer, host: Ipv4Addr, host_port: u16) -> Option<Transmit> {
    (offer.mode != Mode::Rx).then(|| Transmit {
        peer: host.octets(),
        peer_port: host_port,
        payload: offer.payload as u16,
        rate_bps: offer.each.device_bps(),
        burst: offer.burst,
    })
}

/// One level: `offer` over the window, with RTT probes from the start to
/// the end of the drain when it asks and its ping within the window. The
/// access point's view of the station is read before the run and after its
/// drain. Returns the ping's echoes, empty without one.
fn level(cx: &mut Cx<'_>, path: &Path, offer: &Offer) -> Result<traffic::Echoes, Failure> {
    if !diagnostics::requested().map_err(Failure::Incomplete)? {
        return measured_level(cx, path, offer, false);
    }
    let mut switch = LevelSwitch {
        cx,
        name: &offer.name,
    };
    diagnostics::around(&mut switch, |switch| {
        measured_level(switch.cx, path, offer, true)
    })
}

/// The diagnostics' switch of one level, over its context.
struct LevelSwitch<'c, 'a> {
    cx: &'c mut Cx<'a>,
    name: &'c str,
}

impl diagnostics::Switch for LevelSwitch<'_, '_> {
    fn switch(&mut self, on: bool) -> Result<(), Failure> {
        // Through the context: after a request of unknown outcome it sends
        // nothing more, and this fails.
        let _was_on: oer_hil_services_load::Diagnosed =
            self.cx.call(Diagnose { on, reset: false }, CALL)?;
        Ok(())
    }

    fn record(&mut self, off: &Result<(), Failure>) {
        self.cx.measure(
            format!("{} diagnostics off", self.name),
            diagnostics::switched_off(off),
        );
    }
}

/// [`level`]'s run and record; `diagnosing` when its diagnostics are on, to
/// collect them after the window.
fn measured_level(
    cx: &mut Cx<'_>,
    path: &Path,
    offer: &Offer,
    diagnosing: bool,
) -> Result<traffic::Echoes, Failure> {
    let Path {
        device,
        ref station,
        host,
    } = *path;
    let (mode, each, probing) = (offer.mode, offer.each, offer.probing);
    let profile = profile::requested()?;
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
    let transmit = device_transmit(offer, host, host_port);
    let start_call_us = endpoint.now_us();
    let started = cx.call(
        Start {
            run,
            port: DEVICE_PORT,
            delay_ms: DELAY_MS,
            synchronised: true,
            duration_ms: window_ms()?,
            drain_ms: DRAIN_MS,
            transmit,
            profile,
            tune: offer.tune,
        },
        CALL,
    )?;
    let start_answer_us = endpoint.now_us();
    check(
        started == Started::Running,
        format!("the image's load: {started:?}"),
    )?;
    let peer = SocketAddrV4::new(device, DEVICE_PORT);
    let stop = AtomicBool::new(false);
    let window = Duration::from_millis(u64::from(window_ms()?));
    let (sent, probes, echoes, report, begun, begin_call_us, begin_answer_us) =
        std::thread::scope(|scope| {
            let _stop_on_error = traffic::StopOnDrop(&stop);
            // Start the incoming flow while the image is armed, and keep it
            // going until the image reports its completed window and drain.
            // The device releases the barrier only after it actually delivered
            // this run's load. Host response latency cannot create an empty head
            // or cause a separately timed sender to stop before the window ends.
            let sender = (mode != Mode::Tx).then(|| {
                scope.spawn(|| {
                    traffic::send_until(
                        &endpoint,
                        peer,
                        run,
                        offer.payload,
                        each,
                        offer.burst,
                        window
                            + Duration::from_millis(u64::from(DELAY_MS + DRAIN_MS))
                            + Duration::from_secs(20),
                        &stop,
                    )
                })
            });
            let begin_call_us = endpoint.now_us();
            let waiting = Instant::now();
            let begun = loop {
                match cx.call(
                    Begin {
                        run,
                        receive: mode != Mode::Tx,
                    },
                    CALL,
                )? {
                    ready @ Begun::Ready { .. } => break ready,
                    Begun::WaitingForLoad if waiting.elapsed() < Duration::from_secs(5) => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    other => {
                        return Err(Failure::Incomplete(format!(
                            "the load start barrier: {other:?}"
                        )));
                    }
                }
            };
            let begin_answer_us = endpoint.now_us();
            let probes = probing
                .then(|| scope.spawn(|| traffic::probe(&endpoint, peer, run, PROBE_PERIOD, &stop)));
            let requests = (window.as_millis() / ping::PERIOD.as_millis()) as u32;
            let echoes = offer
                .ping
                .then(|| scope.spawn(move || ping::echo(device, requests)));
            // Begin acknowledges the actual opening, after device preparation.
            std::thread::sleep(window);
            let report = collect_report(cx, run)?;
            stop.store(true, Ordering::Relaxed);
            let sent = match sender {
                Some(sender) => sender
                    .join()
                    .map_err(|_| Failure::Incomplete("host load sender panicked".to_owned()))?,
                None => traffic::Sent::default(),
            };
            let probes = probes.map_or(0, |probes| probes.join().unwrap_or(0));
            let echoes =
                echoes.map(|echoes| echoes.join().unwrap_or_else(|_| Err("ping".to_owned())));
            Ok((
                sent,
                probes,
                echoes,
                report,
                begun,
                begin_call_us,
                begin_answer_us,
            ))
        })?;
    check(
        start_consistent(&begun, &report, mode != Mode::Tx),
        "the device start boundary is inconsistent",
    )?;
    let echoes = echoes.transpose().map_err(Failure::Incomplete)?;
    let after = link::read(cx, station);
    let span = since.elapsed();
    let ended = collect_ended(cx, run, report)?;
    let diagnosed = if diagnosing {
        let collected: DiagnosticsCollected = cx.call(CollectDiagnostics { run }, CALL)?;
        Some(collected)
    } else {
        None
    };
    // Hart 0's samples, which a sampled run always has, and hart 1's,
    // where the image samples hart 1.
    let samples = match profile {
        Some(_) => match profile::collect(cx, run, 0, CALL)? {
            Some(samples) => Some((samples, profile::collect(cx, run, 1, CALL)?)),
            None => {
                return Err(Failure::Fail(
                    "the image sampled no hart 0 in a sampled run".to_owned(),
                ));
            }
        },
        None => None,
    };
    let mut received = endpoint.close();
    received.echoes.sent = probes;
    let name = &offer.name;
    let mut value = measurement(offer, &sent, &received, &ended);
    if let Some(object) = value.as_object_mut() {
        // What the harness built and checked against the running image.
        object.insert("build".to_owned(), build(cx.built()));
        object.insert("start_boundary".to_owned(), serde_json::json!({
            "protocol": "armed-flow-begin",
            "host_clock": "endpoint monotonic microseconds; not synchronised with device",
            "host_start_call_us": start_call_us,
            "host_start_answer_us": start_answer_us,
            "host_begin_call_us": begin_call_us,
            "host_begin_answer_us": begin_answer_us,
            "host_first_send_us": sent.first_us,
            "host_last_send_us": sent.last_us,
            "host_first_receive_us": received.first_us,
            "host_last_receive_us": received.last_us,
            "device_begin": begun,
            "device_armed_us": ended.report.armed_us,
            "device_triggered_us": ended.report.triggered_us,
            "device_window_start_us": ended.report.window_start_us,
            "device_window_end_us": ended.report.window_end_us,
            "device_first_delivery_us": ended.report.received.first_us,
            "device_last_delivery_us": ended.report.received.last_us,
            "device_first_send_us": ended.report.sent.first_us,
            "device_last_send_us": ended.report.sent.last_us,
            "rx_loss_scope": "entire host flow including warm-up and drain; window delivery uses device counters",
        }));
        if let Some(echoes) = &echoes {
            object.insert("ping".to_owned(), rtt(echoes));
        }
        let link = link_measurement(name, &before, &after, span, &ended);
        object.insert("link".to_owned(), link);
        if let Some(traced) = &ended.traced {
            let tracking = tracking_measurement(name, traced);
            object.insert("tracking".to_owned(), tracking);
            let stall = stall_measurement(name, &ended.report, traced);
            object.insert("stall".to_owned(), stall);
            let turns = turns_measurement(name, traced);
            object.insert("network_turns".to_owned(), turns);
            let profile = profile_measurement(name, traced);
            object.insert("turn_profile".to_owned(), profile);
            object.insert("addba".to_owned(), addba_measurement(name, traced));
        }
        if let Some(diagnosed) = &diagnosed {
            println!("  diagnostics {name}: {}", diagnostics::summary(diagnosed));
            object.insert(
                "network_diagnostics".to_owned(),
                diagnostics::window(diagnosed),
            );
        }
    }
    if let (Some((hart0, hart1)), Some(object)) = (samples, value.as_object_mut()) {
        let elf = profile::elf(IMAGE);
        let sites = elf
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|elf| profile::copy_sites(elf));
        let report = |samples: &profile::Collected| {
            let symbols = elf
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|elf| profile::symbolize(elf, &samples.samples));
            let path = elf.clone().unwrap_or_default();
            profile::report(samples, &path, &symbols, &sites)
        };
        object.insert("profile".to_owned(), report(&hart0));
        if let Some(hart1) = hart1 {
            object.insert("profile_hart1".to_owned(), report(&hart1));
        }
    }
    cx.measure(name.clone(), value);
    Ok(echoes.unwrap_or_default())
}

/// The build of the image a window ran on: its features and the
/// instrumentation they compiled, from the bundle the harness built.
fn build(built: &BuildChoice) -> serde_json::Value {
    serde_json::json!({
        "features": built.features,
        "default_features": built.default_features,
        "instrumentation": built.instrumentation,
    })
}

/// Only device-clock boundaries are compared. Host send stamps have their
/// own epoch and cannot establish a cross-clock latency by subtraction.
fn start_consistent(begun: &Begun, report: &Report, receive: bool) -> bool {
    let Begun::Ready {
        at_us,
        first_received_us,
        window_start_us,
    } = *begun
    else {
        return false;
    };
    report.armed_us <= at_us
        && report.triggered_us == at_us
        && report.window_start_us == window_start_us
        && window_start_us >= at_us
        && report.window_end_us > window_start_us
        && (!receive
            || first_received_us
                .is_some_and(|first| first <= at_us && report.received.first_us == Some(first)))
}

/// One level's measurement. Throughput is over the measuring window: the
/// image's window for what it received and sent, the host's own sending
/// time for what the host sent, the image's window for what the host
/// received (the image sends only within it). CPU is the image's, at the
/// window's two ends, the parts' polls and the counters within it.
fn measurement(
    offer: &Offer,
    sent: &traffic::Sent,
    received: &traffic::Received,
    ended: &Ended,
) -> serde_json::Value {
    let report = &ended.report;
    let mode = offer.mode;
    let offered_each = offer.each.mbps().map(round);
    let window = window_us(report) as f64 / 1e6;
    let mut value = serde_json::json!({
        "offered_total_mbps": offer.each.mbps().map(|each| round(each * mode.directions() as f64)),
        "payload_bytes": offer.payload,
        "burst_datagrams": offer.burst,
        "window_s": round(window),
        "rtt": rtt(&received.echoes),
        "cpu": utilisation(&report.cpu_start, &report.cpu_end),
        "polls": ended.traced.as_ref().map(|traced| polls(&traced.parts, window_us(report))),
        "counters": counters(&ended.counters, window_us(report)),
        "mac": ended.traced.as_ref().map(|traced| mac_counts(&traced.mac)),
        "positions": ended.traced.as_ref().map(|traced| positions_counts(&traced.positions)),
        "hart1": hart1(&ended.hart1, window_us(report)),
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
                "unreadable": device.unreadable,
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
    let datagrams = window_datagrams(mode, report);
    value["hart1"]["datagrams"] = datagrams.into();
    value["hart1"]["instructions_per_datagram"] = ended
        .hart1
        .end
        .instructions
        .checked_sub(ended.hart1.start.instructions)
        .filter(|_| datagrams != 0)
        .map(|instructions| round(instructions as f64 / datagrams as f64))
        .into();
    value
}

/// The datagrams of `mode`'s directions that crossed the image within its
/// window: hart 1's counters run over those.
fn window_datagrams(mode: Mode, report: &Report) -> u64 {
    let received = if mode == Mode::Tx {
        0
    } else {
        report.received.window_datagrams
    };
    let sent = if mode == Mode::Rx {
        0
    } else {
        report.sent.datagrams
    };
    received.saturating_add(sent)
}

fn percent(part: u64, whole: u64) -> f64 {
    round(100.0 * part as f64 / whole.max(1) as f64)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_window_is_qualified_only_by_consistent_device_barrier_stamps() {
        let mut report = Report {
            armed_us: 100,
            triggered_us: 300,
            window_start_us: 600,
            window_end_us: 5_000_600,
            received: oer_hil_services_load::Received {
                first_us: Some(200),
                ..Default::default()
            },
            ..Default::default()
        };
        let begun = Begun::Ready {
            at_us: 300,
            first_received_us: Some(200),
            window_start_us: 600,
        };
        assert!(start_consistent(&begun, &report, true));
        report.triggered_us = 200;
        assert!(!start_consistent(&begun, &report, true));
        report.triggered_us = 300;
        report.received.first_us = Some(700);
        assert!(!start_consistent(&begun, &report, true));
        // TX-only requires the receiver, not an incoming load datagram.
        let tx = Begun::Ready {
            at_us: 300,
            first_received_us: None,
            window_start_us: 600,
        };
        assert!(start_consistent(&tx, &report, false));
        assert!(!start_consistent(&tx, &report, true));
    }

    #[test]
    fn the_ping_period_meets_every_sparse_shape_at_as_many_phases_as_its_replies_need() {
        let period = u32::try_from(ping::PERIOD.as_millis()).unwrap();
        for shape in SPARSE {
            let phases: std::collections::BTreeSet<u32> = (0..PING_REPLIES_LEAST as u32)
                .map(|request| request * period % shape.every_ms)
                .collect();
            let least = (shape.every_ms as usize).min(PING_REPLIES_LEAST);
            assert_eq!(phases.len(), least, "{}", shape.name());
        }
    }

    #[test]
    fn a_report_with_an_unreadable_lent_datagram_fails_the_case_by_name() {
        let mut report = Report::default();
        assert!(received_readable(&report).is_ok());
        report.received.unreadable = 1;
        match received_readable(&report) {
            Err(Failure::Fail(reason)) => assert!(reason.starts_with(UNREADABLE), "{reason}"),
            other => panic!("an unreadable loan fails the case: {other:?}"),
        }
    }

    #[test]
    fn a_ping_verdict_needs_its_least_replies_and_then_its_median_decides() {
        let echoes = |rtt_us: Vec<u64>| traffic::Echoes { sent: 44, rtt_us };
        let few = ping_within_limit("few", &echoes(vec![1000; PING_REPLIES_LEAST - 1]));
        assert!(matches!(few, Err(Failure::Incomplete(_))), "{few:?}");
        // Few and slow is incomplete too, not a failure.
        let slow_few = ping_within_limit("slow", &echoes(vec![9000; 10]));
        assert!(matches!(slow_few, Err(Failure::Incomplete(_))));
        assert!(ping_within_limit("quick", &echoes(vec![2000; PING_REPLIES_LEAST])).is_ok());
        let slow = ping_within_limit("slow", &echoes(vec![3001; PING_REPLIES_LEAST]));
        assert!(matches!(slow, Err(Failure::Fail(_))), "{slow:?}");
    }

    use super::*;

    #[test]
    fn the_width_is_ht20_ht40_or_he20_and_nothing_else() {
        assert_eq!(parse_width("ht20").unwrap(), RouterChannelWidth::Ht20);
        assert_eq!(parse_width(" ht40 ").unwrap(), RouterChannelWidth::Ht40);
        assert_eq!(parse_width("he20").unwrap(), RouterChannelWidth::He20);
        assert!(parse_width("40").is_err());
    }

    #[test]
    fn a_sparse_shape_offers_its_datagrams_over_its_period() {
        let shape = Sparse {
            datagrams: 2,
            every_ms: 100,
            payload: 1000,
        };
        assert_eq!(shape.rate_bps(), 160_000);
        assert_eq!(shape.name(), "sparse 2 of 1000 bytes every 100 ms");
        assert!(SPARSE.iter().all(|shape| shape.rate_bps() > 0));
        assert!(
            SPARSE
                .iter()
                .all(|shape| u32::try_from(shape.rate_bps()).is_ok())
        );
    }

    #[test]
    fn a_level_splits_its_total_over_its_directions() {
        assert_eq!(
            Offer::level(Mode::RxTx, 130, true).unwrap().each,
            traffic::Rate::paced(65_000_000).unwrap()
        );
        assert_eq!(
            Offer::level(Mode::Rx, 130, true).unwrap().each,
            traffic::Rate::paced(130_000_000).unwrap()
        );
        assert!(Offer::level(Mode::Rx, 5000, true).is_err());
    }

    #[test]
    fn level_zero_is_unpaced_in_every_direction_the_mode_has() {
        let host = Ipv4Addr::new(10, 231, 0, 2);
        for mode in [Mode::Rx, Mode::Tx, Mode::RxTx] {
            let offer = Offer::level(mode, 0, true).unwrap();
            assert_eq!(offer.each, traffic::Rate::Unpaced, "{}", mode.name());
            assert_eq!(offer.each.mbps(), None);
            // The image sends unpaced exactly where the mode has it send.
            let transmit = device_transmit(&offer, host, 40000);
            match mode {
                Mode::Rx => assert_eq!(transmit, None),
                Mode::Tx | Mode::RxTx => {
                    assert_eq!(transmit.map(|transmit| transmit.rate_bps), Some(0));
                }
            }
        }
        // A positive level never turns unpaced, on the wire either.
        let paced = Offer::level(Mode::RxTx, 1, true).unwrap();
        assert_eq!(
            device_transmit(&paced, host, 40000).map(|transmit| transmit.rate_bps),
            Some(500_000)
        );
    }

    #[test]
    fn the_ping_limit_holds_a_median_and_needs_replies() {
        // Fifteen quick, the median, and fourteen slow.
        let echoes = |median: u64| traffic::Echoes {
            sent: 30,
            rtt_us: [vec![900; 15], vec![median], vec![9000; 14]].concat(),
        };
        assert!(ping_within_limit("x", &echoes(2900)).is_ok());
        assert!(matches!(
            ping_within_limit("x", &echoes(3100)),
            Err(Failure::Fail(_))
        ));
        let none = traffic::Echoes {
            sent: 30,
            rtt_us: Vec::new(),
        };
        assert!(matches!(
            ping_within_limit("x", &none),
            Err(Failure::Incomplete(_))
        ));
    }

    #[test]
    fn tunes_name_known_knobs_and_an_empty_set_keeps_the_defaults() {
        let unset = TUNE_UNSET;
        let tunes = parse_tunes(";return-quiet=0,recycled-burst=1;return-quiet=32").unwrap();
        assert_eq!(
            tunes,
            [
                [unset, unset, unset, unset],
                [0, 1, unset, unset],
                [32, unset, unset, unset]
            ]
        );
        assert_eq!(tune_name(&tunes[1]), " return-quiet=0 recycled-burst=1");
        assert_eq!(tune_name(&tunes[0]), "");
        assert!(parse_tunes("nothing=1").is_err());
        assert!(parse_tunes("return-quiet").is_err());
        assert!(parse_tunes("return-quiet=x").is_err());
    }

    #[test]
    fn a_window_is_whole_seconds_within_a_minute() {
        assert_eq!(parse_seconds("1"), Ok(1000));
        assert_eq!(parse_seconds("60"), Ok(60_000));
        assert!(parse_seconds("0").is_err());
        assert!(parse_seconds("61").is_err());
        assert!(parse_seconds("1.5").is_err());
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
            received: oer_hil_services_load::Received {
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
            ..Default::default()
        };
        let traced = Traced {
            parts: Vec::new(),
            link: Link::default(),
            mac: Mac::default(),
            positions: Positions::default(),
            tracking: Tracking::default(),
            stall: Stall::default(),
            stall_mac: StallMac::default(),
            network: NetworkTurns::default(),
            addba: Addba::default(),
            turn_profile: profile_turns::Profile::default(),
        };
        let mut ended = Ended {
            report,
            counters: Counters::default(),
            hart1: Hart1Window::default(),
            traced: Some(traced),
        };
        let offer = Offer::level(Mode::Rx, 65, true).unwrap();
        let value = measurement(&offer, &sent, &traffic::Received::default(), &ended);
        assert_eq!(value["window_s"], 5.0);
        assert_eq!(value["rx"]["device_received_mbps"], 13.0);
        assert!(value.get("tx").is_none());
        assert!(!value["mac"].is_null());

        // The measuring build's window: the same rates and counters, and
        // no count of the trace's.
        ended.traced = None;
        let value = measurement(&offer, &sent, &traffic::Received::default(), &ended);
        assert_eq!(value["rx"]["device_received_mbps"], 13.0);
        assert!(value["counters"].is_object());
        assert!(value["polls"].is_null());
        assert!(value["mac"].is_null());
        assert!(value["positions"].is_null());
    }

    #[test]
    fn only_a_build_without_the_trace_class_is_the_measuring_one() {
        let mut built = BuildChoice {
            features: Vec::new(),
            default_features: true,
            instrumentation: vec![
                "trace".to_owned(),
                "fault-injection".to_owned(),
                "cpu-load".to_owned(),
            ],
        };
        assert!(traced(&built));
        // The measuring build: its idle account and nothing of the trace.
        built.default_features = false;
        built.instrumentation = vec!["cpu-load".to_owned()];
        assert!(!traced(&built));
        built.instrumentation.push("fault-injection".to_owned());
        assert!(!traced(&built));
        let value = build(&built);
        assert_eq!(value["default_features"], false);
        assert_eq!(value["instrumentation"][0], "cpu-load");
    }

    /// Hart 1's window between two ends of (cycles, instructions).
    fn window(start: (u64, u64), end: (u64, u64)) -> Hart1Window {
        let at = |(cycles, instructions)| oer_hil_services_load::HartCount {
            cycles,
            instructions,
        };
        Hart1Window {
            start: at(start),
            end: at(end),
        }
    }

    #[test]
    fn hart_1_s_counters_give_its_clock_and_rate_over_the_window() {
        let value = hart1(
            &window(
                (9_000_000_000, 700_000_000),
                (10_600_000_000, 1_020_000_000),
            ),
            5_000_000,
        );
        assert_eq!(value["advanced"], true);
        assert_eq!(value["cycles"], 1_600_000_000_u64);
        assert_eq!(value["instructions"], 320_000_000_u64);
        assert_eq!(value["cycles_per_us"], 320.0);
        assert_eq!(value["instructions_per_cycle"], 0.2);
        assert_eq!(value["start"]["instructions"], 700_000_000_u64);
    }

    #[test]
    fn a_counter_of_hart_1_that_went_back_or_stood_still_is_no_count() {
        let back = hart1(&window((100, 50), (90, 60)), 5_000_000);
        assert_eq!(back["advanced"], false);
        assert!(back["cycles"].is_null());
        assert!(back["instructions_per_cycle"].is_null());
        assert_eq!(back["instructions"], 10);

        let stood = hart1(&window((100, 50), (200, 50)), 5_000_000);
        assert_eq!(stood["advanced"], false);
        assert_eq!(stood["instructions"], 0);

        let none = hart1(&Hart1Window::default(), 5_000_000);
        assert_eq!(none["advanced"], false);
        assert!(none["instructions_per_cycle"].is_null());
    }

    #[test]
    fn hart_1_s_instructions_are_rated_over_the_mode_s_window_datagrams() {
        let report = Report {
            received: oer_hil_services_load::Received {
                window_datagrams: 300,
                // Those after the window ran outside the counters' ends.
                datagrams: 330,
                ..Default::default()
            },
            sent: oer_hil_services_load::Sent {
                datagrams: 100,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(window_datagrams(Mode::Rx, &report), 300);
        assert_eq!(window_datagrams(Mode::Tx, &report), 100);
        assert_eq!(window_datagrams(Mode::RxTx, &report), 400);

        let sent = traffic::Sent {
            datagrams: 300,
            bytes: 0,
            failed: 0,
            elapsed: Duration::from_secs(1),
            ..Default::default()
        };
        // Hart 1's counters come from both builds: the measuring one's.
        let ended = Ended {
            report,
            counters: Counters::default(),
            hart1: window((0, 0), (320_000_000, 3_000_000)),
            traced: None,
        };
        let offer = Offer::level(Mode::Rx, 65, true).unwrap();
        let value = measurement(&offer, &sent, &traffic::Received::default(), &ended);
        assert_eq!(value["hart1"]["datagrams"], 300);
        assert_eq!(value["hart1"]["instructions_per_datagram"], 10_000.0);
    }

    #[test]
    fn a_hart_s_busy_share_is_its_window_less_its_idle_time() {
        let at = |now_us, idle_us, work_us, count| Load {
            now_us,
            harts: [oer_hil_services_load::HartLoad {
                hart: 0,
                idle_us,
                wakes: count,
            }]
            .into_iter()
            .collect(),
            executors: [oer_hil_services_load::ExecutorLoad {
                executor: 1,
                hart: 0,
                level: 2,
                work_us,
                rounds: count,
            }]
            .into_iter()
            .collect(),
        };
        let value = utilisation(
            &at(1_000_000, 400_000, 0, u32::MAX),
            &at(2_000_000, 1_000_000, 300_000, 2_999),
        );
        assert_eq!(value["harts"][0]["busy_percent"], 40.0);
        assert_eq!(value["executors"][0]["work_percent"], 30.0);
        assert_eq!(
            value["harts"][0]["wakes_per_s"], 3000.0,
            "a wake count that wrapped still differs"
        );
        assert_eq!(value["executors"][0]["rounds_per_s"], 3000.0);
    }

    #[test]
    fn the_counters_give_the_clock_the_instructions_per_cycle_and_per_stack_frame() {
        let value = counters(
            &Counters {
                cycles: 1_600_000_000,
                instructions: 480_000_000,
                stack_received: 500,
                ..Counters::default()
            },
            5_000_000,
        );
        assert_eq!(value["cycles_per_us"], 320.0);
        assert_eq!(value["instructions_per_cycle"], 0.3);
        assert_eq!(value["instructions_per_stack_frame"], 960_000.0);
        let traced = Counters {
            rx: Some(oer_hil_services_load::RxCounts::default()),
            tx: Some(oer_hil_services_load::TxCounts::default()),
            ..Counters::default()
        };
        let zeros = counters(&traced, 5_000_000);
        assert_eq!(zeros["rx"]["driver"]["available"], false);
        assert_eq!(zeros["rx"]["station"]["agreement_window"], 0);
        assert_eq!(zeros["tx"]["ampdu"]["aggregates"], 0);
        assert!(zeros["tx"]["ampdu"]["average_subframes"].is_null());
        // The measuring build counts neither path: no count, not zeros.
        let none = counters(&Counters::default(), 5_000_000);
        assert!(none["rx"].is_null());
        assert!(none["tx"].is_null());
        let sent = counters(
            &Counters {
                tx: Some(oer_hil_services_load::TxCounts {
                    ampdu: oer_hil_services_load::TxAmpdu {
                        aggregates: 4,
                        subframes: 90,
                        subframes_17_32: 3,
                        ..oer_hil_services_load::TxAmpdu::default()
                    },
                }),
                ..Counters::default()
            },
            5_000_000,
        );
        assert_eq!(sent["tx"]["ampdu"]["average_subframes"], 22.5);
        assert_eq!(sent["tx"]["ampdu"]["subframes_17_32"], 3);
        assert!(none["instructions_per_cycle"].is_null());
        assert!(none["instructions_per_stack_frame"].is_null());
    }

    #[test]
    fn a_bus_s_misses_are_rated_per_thousand_instructions_and_per_stack_frame() {
        let value = counters(
            &Counters {
                instructions: 4_000_000,
                stack_received: 4000,
                cache: oer_hil_services_load::CacheCounts {
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
                    ..oer_hil_services_load::CacheCounts::default()
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
                cache: oer_hil_services_load::CacheCounts {
                    dbus1: DataBus {
                        hits: 10,
                        misses: 10,
                        overflowed: true,
                        ..DataBus::default()
                    },
                    ..oer_hil_services_load::CacheCounts::default()
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
        let part = oer_hil_services_load::PartWindow {
            name: "ip".try_into().unwrap(),
            polls: 2000,
            poll_us: 1_500_000,
            ended_poll_us: 1_500_000,
            longest_poll_us: Some(900),
        };
        let idle = oer_hil_services_load::PartWindow {
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
        let part = oer_hil_services_load::PartWindow {
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
        assert_eq!(parse_levels("0").ok(), Some(vec![0]), "as fast as it goes");
        for refused in ["", "1001", "18446744073709551615", "10,x"] {
            assert!(parse_levels(refused).is_err(), "{refused}");
        }
        assert!(u32::try_from(MAX_LEVEL_MBPS * 1_000_000).is_ok());
    }
}
