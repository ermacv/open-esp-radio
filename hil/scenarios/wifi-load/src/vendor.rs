//! The RX levels against the vendor receiver: the stand's ESP-IDF UDP
//! receiver peer (`tools/lab/peers/esp32s31-wifi-udp-rx`) on a board of the
//! stand instead of the image, so the vendor driver and the open station take
//! the same load through the same access point.
//! `OER_WIFI_LOAD_VENDOR=socket|fast` runs this instead of the cases with the
//! peer's [`Variant`]; the program's arguments (`--list`, `--case`, `--board`)
//! are read before anything reaches the stand.
//!
//! Under one lease of the board, the router and the air exclusively (the
//! cases' claims), the run writes the built peer unless the board runs it,
//! opens its console with a reset, applies the router's station profile
//! with a generated network, hands the peer that network in one `JOIN` line
//! (`path`) and runs each RX level (`rx`): the access point's view before
//! and after it, the host's load and probes as in [`crate::level`], and the
//! peer's report of that run. The peer's counts by the load header and its
//! counters over the window (snapshots at the window's start and end, as
//! the image counts its own) and over the run (from its start to its drain's
//! end, the access point's span) are mandatory: a missing field, or a report
//! of another run, ends the run as incomplete. The generated network's name
//! and passphrase stay in memory: they go to the router and to the peer's
//! console input only, and every line kept of the peer's console has them
//! removed. Results go to
//! `target/hil/results/wifi-load-vendor-<time in ns>.json`.

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    process::ExitCode,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use oer_tools_hil_harness::{Failure, RouterChannelWidth, RouterOperation};
use oer_tools_lab_arbiter::{
    claims::{AIR, Claim},
    journal::ImageIdentity,
    readiness::ROUTER,
};
use oer_tools_lab_client::{
    client::Client,
    protocol::{FlashVia, LeaseRequest, Operation},
};
use oer_tools_lab_model::peer_line::{self, Line};
use serde_json::{Map, Value, json};

use crate::{
    DELAY_MS, DEVICE_PORT, DRAIN_MS, Mode, Offer, PROBE_PERIOD, ROUTER_ADDRESS, channel, generated,
    host_route, levels_mbps, link, mbps, percent, probes, round, rtt, run_id, traffic, width,
    window_ms,
};

/// Selects the vendor receiver and its variant.
pub const VARIANT_ENV: &str = "OER_WIFI_LOAD_VENDOR";
/// The peer's line protocol.
const PROTOCOL: u32 = 2;
/// How long the peer's boot may take to say `@READY` after the reset.
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// The peer's own join wait is 30 s.
const JOIN_TIMEOUT: Duration = Duration::from_secs(40);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// The peer's REPORT waits for the run's confirmed end, up to 3 s past it.
const REPORT_TIMEOUT: Duration = Duration::from_secs(8);
/// The cases of a vendor run, in order: `path` joins and checks the route,
/// `rx` runs the levels on that join.
const CASES: [&str; 2] = ["path", "rx"];

/// How the peer consumes the load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Variant {
    /// A receiver task reads a BSD socket (`wifi-udp-rx-peer`).
    Socket,
    /// lwIP's thread counts through the raw UDP API and frees each buffer
    /// at once (`wifi-udp-rx-fast-peer`).
    Fast,
}

impl Variant {
    fn image(self) -> &'static str {
        match self {
            Self::Socket => "wifi-udp-rx-peer",
            Self::Fast => "wifi-udp-rx-fast-peer",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Socket => "socket",
            Self::Fast => "fast",
        }
    }
}

fn parse_variant(text: &str) -> Result<Variant, String> {
    match text.trim() {
        "socket" => Ok(Variant::Socket),
        "fast" => Ok(Variant::Fast),
        other => Err(format!(
            "{VARIANT_ENV}=`{other}` is neither `socket` nor `fast`"
        )),
    }
}

/// The variant `OER_WIFI_LOAD_VENDOR` asks for; `None` when it is unset.
pub fn requested() -> Result<Option<Variant>, String> {
    match std::env::var(VARIANT_ENV) {
        Err(_) => Ok(None),
        Ok(text) => parse_variant(&text).map(Some),
    }
}

/// The command line of a vendor run: the cases' own.
#[derive(clap::Parser, Debug)]
struct Arguments {
    /// The board the peer runs on, by its stand-file id or MAC; without it,
    /// the one ESP32-S31 board with the `dut` role.
    #[arg(long)]
    board: Option<String>,
    /// Run only this case (`path`, `rx`). May be repeated; `rx` joins first.
    #[arg(long = "case")]
    cases: Vec<String>,
    /// Print the cases and exit.
    #[arg(long)]
    list: bool,
}

/// What a vendor run does, read from its arguments and environment before
/// it reaches the stand.
#[derive(Debug, PartialEq)]
struct Plan {
    variant: Variant,
    board: Option<String>,
    /// Whether the levels run after the join.
    rx: bool,
    levels: Vec<u64>,
    window_ms: u32,
    probing: bool,
    width: RouterChannelWidth,
}

/// Whether `names` select the levels; an unknown case is an error.
fn select(names: &[String]) -> Result<bool, String> {
    if let Some(unknown) = names.iter().find(|name| !CASES.contains(&name.as_str())) {
        return Err(format!("the vendor run has no case `{unknown}`"));
    }
    Ok(names.is_empty() || names.iter().any(|name| name == "rx"))
}

impl Plan {
    fn of(variant: Variant, arguments: &Arguments) -> Result<Self, String> {
        Ok(Self {
            variant,
            board: arguments.board.clone(),
            rx: select(&arguments.cases)?,
            levels: levels_mbps().map_err(failure)?,
            window_ms: window_ms().map_err(failure)?,
            probing: probes().map_err(failure)?,
            width: width().map_err(failure)?,
        })
    }
}

/// Run the vendor receiver `variant` as the arguments say: 0 when every
/// selected case ran, 2 when the run could not finish or its arguments are
/// wrong. `--list` and an argument error never reach the stand.
pub fn main(variant: Variant) -> ExitCode {
    let arguments = <Arguments as clap::Parser>::parse();
    if arguments.list {
        for case in CASES {
            println!("{case}");
        }
        return ExitCode::SUCCESS;
    }
    let plan = match Plan::of(variant, &arguments) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    let mut measurements = Vec::new();
    let mut board_id = plan.board.clone().unwrap_or_default();
    let ended = run(&plan, &mut board_id, &mut measurements);
    let verdict = match &ended {
        Ok(()) => "pass",
        Err(reason) => {
            eprintln!("vendor: {reason}");
            "incomplete"
        }
    };
    if !measurements.is_empty() {
        write_results(&plan, &board_id, &measurements, &ended, verdict);
    }
    if ended.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}

/// The board `selector` names, or the one ESP32-S31 board with the `dut`
/// role.
fn board<'a>(
    stand: &'a oer_tools_lab_model::Stand,
    selector: Option<&str>,
) -> Result<&'a oer_tools_lab_model::Board, String> {
    if let Some(selector) = selector {
        return stand
            .board(selector)
            .ok_or_else(|| format!("the stand file has no board `{selector}`"));
    }
    let chip = oer_tools_image_build::images::CHIP;
    let candidates = stand
        .boards
        .iter()
        .filter(|board| board.chip == chip && board.has_role(oer_tools_lab_model::BoardRole::Dut))
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [board] => Ok(board),
        [] => Err(format!(
            "the stand file has no {chip} board with the dut role"
        )),
        _ => Err(format!(
            "the stand file has several {chip} boards with the dut role; name one with --board"
        )),
    }
}

fn run(
    plan: &Plan,
    board_id: &mut String,
    measurements: &mut Vec<(String, Value)>,
) -> Result<(), String> {
    let root = oer_tools_os_process::Checkout::discover("HIL scenario")
        .map_err(|error| error.to_string())?
        .root;
    let image = plan.variant.image();
    let bundle = bundle(&root, image)?;
    let stand = crate::stand().map_err(failure)?;
    let board = board(&stand, plan.board.as_deref())?;
    board_id.clone_from(&board.id);
    let mac = board.mac.clone();
    let owner = oer_tools_lab_arbiter::owner::from_environment()
        .map_err(|error| error.to_string())?
        .to_string();
    let mut client = Client::open("hil wifi-load vendor")?;
    eprintln!(
        "vendor: waiting for board {}, the router and the air",
        board.id
    );
    client.acquire(
        LeaseRequest {
            owner: owner.clone(),
            work: format!("hil wifi-load vendor {image}"),
            scenarios: vec!["wifi-load".to_owned()],
            run: None,
            claims: vec![
                Claim::board(&mac),
                Claim::exclusive(ROUTER),
                Claim::exclusive(AIR),
            ],
            maintenance: None,
            divisible: false,
        },
        &mut |message| eprintln!("vendor: {message}"),
    )?;
    let ran = leased(
        &root,
        plan,
        &mut client,
        &mac,
        &owner,
        &bundle,
        measurements,
    );
    let released = client.release();
    ran.and(released)
}

/// The peer's image bundle, from its current catalog build.
fn bundle(root: &std::path::Path, image: &str) -> Result<std::path::PathBuf, String> {
    use oer_tools_image_build::esp_idf::catalog;
    let build = catalog::current(root, image).map_err(text)?;
    let entries = catalog::entries(root).map_err(text)?;
    let entry = catalog::entry(&entries, image).map_err(text)?;
    let output = oer_tools_image_esp_idf::output(root, &entry.project(root)).join("bundle");
    let bundle = catalog::bundle(root, entry, &build, &output).map_err(text)?;
    eprintln!(
        "vendor: {image} {} ({})",
        build.application_sha256,
        bundle.directory().display()
    );
    Ok(bundle.directory().to_owned())
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn failure(failure: Failure) -> String {
    match failure {
        Failure::Fail(reason) | Failure::Incomplete(reason) => reason,
    }
}

fn leased(
    root: &std::path::Path,
    plan: &Plan,
    client: &mut Client,
    mac: &str,
    owner: &str,
    bundle: &std::path::Path,
    measurements: &mut Vec<(String, Value)>,
) -> Result<(), String> {
    let image = plan.variant.image();
    let git = |arguments: &[&str]| oer_tools_os_process::git::text(root, arguments).ok();
    let written = client.run(
        Operation::Flash {
            board: mac.to_owned(),
            bundle: bundle.display().to_string(),
            image: ImageIdentity {
                name: image.to_owned(),
                sha256: String::new(),
                commit: git(&["rev-parse", "HEAD"]),
                dirty: git(&["status", "--porcelain"]).map(|status| !status.is_empty()),
                origin: std::env::args().collect::<Vec<_>>().join(" "),
            },
            owner: owner.to_owned(),
            via: FlashVia::Usb,
            skip_carried: true,
        },
        &mut std::io::sink(),
        None,
    )?;
    eprintln!(
        "vendor: {}",
        if written.get("written") == Some(&Value::Bool(false)) {
            "the board runs the peer already"
        } else {
            "wrote the peer"
        }
    );
    let console = client.start(Operation::Console {
        board: mac.to_owned(),
        reset: true,
    })?;
    let mut peer = Peer::new(console);
    let ready = peer.ready(plan.variant)?;
    let address = ready
        .iter()
        .find_map(|report| {
            (report.name == "ADDR")
                .then(|| report.positional(0))
                .flatten()
        })
        .ok_or("the peer named no station address")?
        .to_owned();
    let router = |client: &mut Client, request: RouterOperation| {
        client.run(Operation::Router { request }, &mut std::io::sink(), None)
    };
    let host = host_route(Ipv4Addr::from(ROUTER_ADDRESS))
        .map_err(failure)?
        .source;
    let channel = channel().map_err(failure)?;
    let (ssid, passphrase) = (generated("oer-load-"), generated("oer-"));
    peer.secrets = vec![ssid.clone(), passphrase.clone()];
    router(
        client,
        RouterOperation::ApplyStationProfile {
            ssid: ssid.clone(),
            passphrase: passphrase.clone(),
            channel,
            width: plan.width,
            forward_host: Some(host),
        },
    )?;
    let joined = peer.command(&format!("JOIN {channel} {ssid} {passphrase}"), JOIN_TIMEOUT)?;
    let joined = joined
        .iter()
        .find(|report| report.name == "JOINED")
        .ok_or("the peer joined without its JOINED report")?;
    let device: Ipv4Addr = joined
        .field("ip")
        .and_then(|ip| ip.parse().ok())
        .ok_or("the peer's JOINED report names no address")?;
    let route = host_route(device).map_err(failure)?;
    if route.source != host {
        return Err(format!(
            "the host reaches the peer from {}, the router forwards {host}",
            route.source
        ));
    }
    measure(
        measurements,
        "vendor path",
        json!({
            "topology": "host -> Ethernet -> OpenWrt AP -> Wi-Fi -> vendor station",
            "image": image,
            "variant": plan.variant.name(),
            "host": host.to_string(),
            "device": device.to_string(),
            "joined": fields(joined),
        }),
    );
    if plan.rx {
        for &total in &plan.levels {
            let offer = Offer::level(Mode::Rx, total, plan.probing).map_err(failure)?;
            level(
                client,
                &mut peer,
                plan,
                &address,
                device,
                host,
                &offer,
                measurements,
            )?;
        }
    }
    peer.close();
    Ok(())
}

/// The value of `key` in `report`, which it must hold.
fn required<T: std::str::FromStr>(report: &peer_line::Report, key: &str) -> Result<T, String> {
    report
        .parsed(key)
        .ok_or_else(|| format!("the peer's {} report holds no valid `{key}`", report.name))
}

/// The report `name` (of `span`, when given) among `reports`, which must
/// be of `run` and hold each of `keys`.
fn report_of<'a>(
    reports: &'a [peer_line::Report],
    run: u32,
    name: &str,
    span: Option<&str>,
    keys: &[&str],
) -> Result<&'a peer_line::Report, String> {
    let what = span.map_or_else(|| name.to_owned(), |span| format!("{name} span={span}"));
    let report = reports
        .iter()
        .find(|report| {
            report.name == name && span.is_none_or(|span| report.field("span") == Some(span))
        })
        .ok_or_else(|| format!("the peer's report holds no {what} line"))?;
    let of: u32 = required(report, "run")?;
    if of != run {
        return Err(format!("the peer's {what} line is of run {of}, not {run}"));
    }
    if let Some(error) = report.field("error") {
        return Err(format!("the peer's {what} line reports error {error}"));
    }
    for key in keys {
        required::<i64>(report, key).map_err(|error| format!("{error} ({what})"))?;
    }
    Ok(report)
}

/// The fields the comparison reads of each report line.
const RX_KEYS: [&str; 15] = [
    "datagrams",
    "bytes",
    "window_datagrams",
    "window_bytes",
    "lost",
    "reordered",
    "duplicates",
    "untracked",
    "highest",
    "foreign",
    "echoed",
    "echo_refused",
    "receive_errors",
    "window_us",
    "last_us",
];
const SPAN_KEYS: [&str; 3] = ["window_from_us", "window_to_us", "run_to_us"];
const LWIP_KEYS: [&str; 4] = ["ip_recv", "ip_drop", "udp_recv", "udp_drop"];
const WIFI_RX_KEYS: [&str; 3] = ["ht", "ht_noeb", "ht_retry"];
const HW_RX_KEYS: [&str; 4] = ["rx_fcs_err", "rx_mpdu", "rx_buf_fullcnt", "rx_fifo_ovfcnt"];

/// The peer's report of `run`: every mandatory line and field, all of
/// `run`.
struct Reported<'a> {
    rx: &'a peer_line::Report,
    span: &'a peer_line::Report,
    lwip_window: &'a peer_line::Report,
    lwip_run: &'a peer_line::Report,
    wifi_rx_window: &'a peer_line::Report,
    wifi_rx_run: &'a peer_line::Report,
    hw_rx_window: &'a peer_line::Report,
    link: &'a peer_line::Report,
}

impl<'a> Reported<'a> {
    fn of(reports: &'a [peer_line::Report], run: u32) -> Result<Self, String> {
        Ok(Self {
            rx: report_of(reports, run, "RX", None, &RX_KEYS)?,
            span: report_of(reports, run, "SPAN", None, &SPAN_KEYS)?,
            lwip_window: report_of(reports, run, "LWIP", Some("window"), &LWIP_KEYS)?,
            lwip_run: report_of(reports, run, "LWIP", Some("run"), &LWIP_KEYS)?,
            wifi_rx_window: report_of(reports, run, "WIFIRX", Some("window"), &WIFI_RX_KEYS)?,
            wifi_rx_run: report_of(reports, run, "WIFIRX", Some("run"), &WIFI_RX_KEYS)?,
            hw_rx_window: report_of(reports, run, "HWRX", Some("window"), &HW_RX_KEYS)?,
            link: report_of(reports, run, "LINK", None, &[])?,
        })
    }
}

/// The access point's MPDUs to the station against what the peer's driver
/// counted over the same span (the run's); what neither closes stays as
/// `unclosed`.
fn balance(router: &Value, reported: &Reported<'_>) -> Result<Value, String> {
    let ht: u64 = required(reported.wifi_rx_run, "ht")?;
    let no_buffer: u64 = required(reported.wifi_rx_run, "ht_noeb")?;
    let udp: u64 = required(reported.lwip_run, "udp_recv")?;
    let counted: u64 = required(reported.rx, "datagrams")?;
    let ap = router.pointer("/to_device/packets").and_then(Value::as_u64);
    let failed = router.pointer("/to_device/failed").and_then(Value::as_u64);
    Ok(json!({
        "span": "run: the access point's readings before the RX command and after the drain; the peer's counters from its RX to its drain's end",
        "ap_packets": ap,
        "ap_failed": failed,
        "driver_ht_mpdus": ht,
        "ap_minus_driver": ap.map(|ap| i128::from(ap) - i128::from(ht)),
        "driver_no_buffer": no_buffer,
        "driver_passed": ht.saturating_sub(no_buffer),
        "ip_udp_received": udp,
        "load_counted": counted,
        "unclosed": ap.map(|ap| i128::from(ap) - i128::from(ht)).filter(|difference| *difference != 0),
    }))
}

/// One RX level against the peer, as [`crate::level`] runs it against the
/// image.
#[allow(clippy::too_many_arguments)]
fn level(
    client: &mut Client,
    peer: &mut Peer,
    plan: &Plan,
    station: &str,
    device: Ipv4Addr,
    host: Ipv4Addr,
    offer: &Offer,
    measurements: &mut Vec<(String, Value)>,
) -> Result<(), String> {
    let read = |client: &mut Client| -> link::Reading {
        client.run(
            Operation::Router {
                request: RouterOperation::StationLink {
                    mac: station.to_owned(),
                },
            },
            &mut std::io::sink(),
            None,
        )
    };
    let run = run_id();
    let window = plan.window_ms;
    let endpoint = traffic::Endpoint::open(SocketAddrV4::new(host, 0), run)
        .map_err(|error| format!("the host socket: {error}"))?;
    let before = read(client);
    let since = Instant::now();
    peer.command(
        &format!("RX {run} {DEVICE_PORT} {DELAY_MS} {window} {DRAIN_MS}"),
        COMMAND_TIMEOUT,
    )?;
    let target = SocketAddrV4::new(device, DEVICE_PORT);
    let stop = AtomicBool::new(false);
    let (sent, probes) = std::thread::scope(|scope| {
        std::thread::sleep(Duration::from_millis(u64::from(DELAY_MS)));
        let probes = offer
            .probing
            .then(|| scope.spawn(|| traffic::probe(&endpoint, target, run, PROBE_PERIOD, &stop)));
        let sent = traffic::send(
            &endpoint,
            target,
            run,
            offer.payload,
            offer.each_bps,
            offer.burst,
            Duration::from_millis(u64::from(window)),
        );
        stop.store(true, Ordering::Relaxed);
        let probes = probes.map_or(Ok(0), |probes| {
            probes
                .join()
                .map_err(|_| "the probe thread failed".to_owned())
        });
        std::thread::sleep(Duration::from_millis(u64::from(DRAIN_MS)));
        (sent, probes)
    });
    let probes = probes?;
    let after = read(client);
    let span = since.elapsed();
    let reports = peer.command(&format!("REPORT {run}"), REPORT_TIMEOUT)?;
    let reported = Reported::of(&reports, run)?;
    let dump = peer.dump()?;
    let mut received = endpoint.close();
    received.echoes.sent = probes;
    let rx = reported.rx;
    let window_s = required::<u64>(rx, "window_us")? as f64 / 1e6;
    let datagrams: u64 = required(rx, "datagrams")?;
    let router = link::router_window(&before, &after, span);
    let value = json!({
        "target": "vendor",
        "image": plan.variant.image(),
        "variant": plan.variant.name(),
        "offered_total_mbps": round(offer.each_bps as f64 / 1e6),
        "payload_bytes": offer.payload,
        "burst_datagrams": offer.burst,
        "window_s": round(window_s),
        "rtt": rtt(&received.echoes),
        "rx": {
            "offered_mbps": round(offer.each_bps as f64 / 1e6),
            "host_sent_mbps": mbps(sent.bytes, sent.elapsed.as_secs_f64()),
            "host_sent_datagrams": sent.datagrams,
            "host_send_failed": sent.failed,
            "device_received_mbps": mbps(required(rx, "window_bytes")?, window_s),
            "device_received_datagrams": datagrams,
            "device_received_in_window": required::<u64>(rx, "window_datagrams")?,
            "lost": sent.datagrams.saturating_sub(datagrams),
            "lost_percent": percent(sent.datagrams.saturating_sub(datagrams), sent.datagrams),
            "lost_below_highest": required::<u64>(rx, "lost")?,
            "reordered": required::<u64>(rx, "reordered")?,
            "duplicates": required::<u64>(rx, "duplicates")?,
            "untracked": required::<u64>(rx, "untracked")?,
            "foreign": required::<u64>(rx, "foreign")?,
        },
        "link": { "router": router },
        "vendor": {
            "rx": fields(rx),
            "span": fields(reported.span),
            "window": {
                "lwip": fields(reported.lwip_window),
                "wifi_rx": fields(reported.wifi_rx_window),
                "hw_rx": fields(reported.hw_rx_window),
            },
            "run": {
                "lwip": fields(reported.lwip_run),
                "wifi_rx": fields(reported.wifi_rx_run),
            },
            "balance": balance(&router, &reported)?,
            "link": fields(reported.link),
            "dump_after_drain_since_boot": dump,
        },
    });
    measure(measurements, format!("vendor {}", offer.name), value);
    Ok(())
}

/// A report line's `key=value` fields, numbers as numbers, and its
/// positional fields in `positional`.
fn fields(report: &peer_line::Report) -> Value {
    let mut object = Map::new();
    let mut positional = Vec::new();
    for field in &report.fields {
        match field.split_once('=') {
            Some((key, value)) => {
                let value = value
                    .parse::<i64>()
                    .map(Value::from)
                    .unwrap_or_else(|_| Value::from(value));
                object.insert(key.to_owned(), value);
            }
            None => positional.push(Value::from(field.as_str())),
        }
    }
    if !positional.is_empty() {
        object.insert("positional".to_owned(), positional.into());
    }
    object.into()
}

fn measure(measurements: &mut Vec<(String, Value)>, name: impl Into<String>, value: Value) {
    let name = name.into();
    println!("  measure {name}: {value}");
    measurements.push((name, value));
}

fn write_results(
    plan: &Plan,
    board: &str,
    measurements: &[(String, Value)],
    ended: &Result<(), String>,
    verdict: &str,
) {
    let Ok(checkout) = oer_tools_os_process::Checkout::discover("HIL scenario") else {
        return;
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let results = json!({
        "scenario": "wifi-load",
        "target": "vendor",
        "image": plan.variant.image(),
        "variant": plan.variant.name(),
        "board": board,
        "unix": nanos / 1_000_000_000,
        "reason": ended.as_ref().err(),
        "verdict": verdict,
        "measurements": measurements.iter().map(|(name, value)| json!({
            "case": if name == "vendor path" { "path" } else { "rx" },
            "name": name,
            "value": value,
        })).collect::<Vec<_>>(),
    });
    let directory = checkout.root.join("target/hil/results");
    let path = directory.join(format!("wifi-load-vendor-{nanos}.json"));
    let written = std::fs::create_dir_all(&directory)
        .and_then(|()| std::fs::write(&path, format!("{results:#}\n")));
    match written {
        Ok(()) => println!("results: {}", path.display()),
        Err(error) => eprintln!("vendor: the results were not written: {error}"),
    }
}

/// Writes bytes to a console's input.
type Write = Box<dyn Fn(&[u8]) -> Result<(), String>>;

/// The peer's console: its protocol lines, and its input.
struct Peer {
    lines: mpsc::Receiver<String>,
    /// Writes to the peer's console input.
    write: Write,
    /// Ends the console operation.
    end: Box<dyn Fn()>,
    /// Removed from every line kept of the console.
    secrets: Vec<String>,
}

impl Peer {
    fn new(console: oer_tools_lab_client::client::Running) -> Self {
        let (write, end) = (console.input(), console.input());
        let (send, lines) = mpsc::channel();
        std::thread::spawn(move || {
            use std::io::BufRead as _;
            for line in std::io::BufReader::new(console).split(b'\n') {
                let Ok(line) = line else {
                    break;
                };
                let line = String::from_utf8_lossy(&line).trim_end().to_owned();
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            lines,
            write: Box::new(move |bytes| write.send(bytes)),
            end: Box::new(move || {
                let _ = end.cancel();
            }),
            secrets: Vec::new(),
        }
    }

    fn redact(&self, line: &str) -> String {
        self.secrets.iter().fold(line.to_owned(), |line, secret| {
            line.replace(secret.as_str(), "<redacted>")
        })
    }

    fn next(&self, deadline: Instant) -> Option<String> {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        self.lines
            .recv_timeout(remaining)
            .ok()
            .map(|line| self.redact(&line))
    }

    /// The reports up to the boot's `@READY` of this peer in `variant`.
    fn ready(&mut self, variant: Variant) -> Result<Vec<peer_line::Report>, String> {
        let deadline = Instant::now() + READY_TIMEOUT;
        let mut ready = None;
        while let Some(line) = self.next(deadline) {
            match peer_line::parse(&line) {
                Some(Line::Ready { protocol, report }) => {
                    if protocol != PROTOCOL
                        || report.field("app") != Some("wifi-udp-rx")
                        || report.field("variant") != Some(variant.name())
                    {
                        return Err(format!("the board runs another peer: {line}"));
                    }
                    ready = Some(vec![report]);
                }
                Some(Line::Report(report)) if report.name == "ADDR" => {
                    if let Some(reports) = ready.as_mut() {
                        reports.push(report);
                        return Ok(reports.clone());
                    }
                }
                _ => {}
            }
        }
        Err("the peer did not say @READY with its address after its reset".to_owned())
    }

    /// Send `command` and return its reports up to its `@OK`; its `@ERR`
    /// or silence is an error. The error names only the command's first
    /// word: a JOIN line holds the network's passphrase.
    fn command(
        &mut self,
        command: &str,
        timeout: Duration,
    ) -> Result<Vec<peer_line::Report>, String> {
        let name = command.split(' ').next().unwrap_or_default().to_owned();
        (self.write)(format!("{command}\n").as_bytes())?;
        let deadline = Instant::now() + timeout;
        let mut reports = Vec::new();
        while let Some(line) = self.next(deadline) {
            match peer_line::parse(&line) {
                Some(Line::Ok { command }) if command == name => return Ok(reports),
                Some(Line::Err { command, reason }) if command == name => {
                    return Err(format!("the peer refused {name}: {reason}"));
                }
                Some(Line::Report(report)) => {
                    if report.name == "LOST" {
                        eprintln!("vendor: {line}");
                    }
                    reports.push(report);
                }
                _ => {}
            }
        }
        Err(format!("the peer did not answer {name}"))
    }

    /// The vendor's statistics dump: every console line between its
    /// markers.
    fn dump(&mut self) -> Result<Vec<String>, String> {
        (self.write)(b"DUMP\n")?;
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let mut lines = Vec::new();
        let mut inside = false;
        while let Some(line) = self.next(deadline) {
            match peer_line::parse(&line) {
                Some(Line::Ok { command }) if command == "DUMP" => return Ok(lines),
                Some(Line::Report(report)) if report.name == "DUMP" => {
                    inside = report.positional(0) == Some("begin");
                    if !inside {
                        lines.push(line);
                    }
                }
                _ if inside => lines.push(line),
                _ => {}
            }
        }
        Err("the peer did not end its DUMP".to_owned())
    }

    fn close(&self) {
        (self.end)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A peer whose console says `lines`, recording what the host wrote.
    fn console(lines: &[&str], secrets: &[&str]) -> (Peer, Arc<Mutex<Vec<u8>>>) {
        let (send, receive) = mpsc::channel();
        for line in lines {
            send.send((*line).to_owned()).unwrap();
        }
        let written = Arc::new(Mutex::new(Vec::new()));
        let sink = written.clone();
        let peer = Peer {
            lines: receive,
            write: Box::new(move |bytes| {
                sink.lock().unwrap().extend_from_slice(bytes);
                Ok(())
            }),
            end: Box::new(|| {}),
            secrets: secrets.iter().map(|secret| (*secret).to_owned()).collect(),
        };
        (peer, written)
    }

    #[test]
    fn a_command_returns_its_reports_up_to_its_own_answer() {
        let (mut peer, written) = console(
            &[
                "W (12) wifi: noise",
                "@OK SYNC",
                "@RX run=7 datagrams=3",
                "@HWRX rx_fcs_err=2",
                "@OK REPORT",
                "@RX run=8",
            ],
            &[],
        );
        let reports = peer.command("REPORT 7", Duration::from_secs(1)).unwrap();
        assert_eq!(
            reports
                .iter()
                .map(|report| report.name.as_str())
                .collect::<Vec<_>>(),
            ["RX", "HWRX"]
        );
        assert_eq!(&*written.lock().unwrap(), b"REPORT 7\n");
    }

    #[test]
    fn a_refusal_or_silence_is_an_error_naming_only_the_command() {
        let (mut peer, _) = console(&["@ERR JOIN timeout disconnects=3"], &[]);
        let error = peer
            .command(
                "JOIN 13 oer-load-ssid oer-passphrase",
                Duration::from_secs(1),
            )
            .unwrap_err();
        assert_eq!(error, "the peer refused JOIN: timeout disconnects=3");
        let (mut peer, _) = console(&[], &[]);
        let error = peer
            .command(
                "JOIN 13 oer-load-ssid oer-passphrase",
                Duration::from_millis(10),
            )
            .unwrap_err();
        assert!(
            !error.contains("oer-passphrase") && !error.contains("ssid"),
            "{error}"
        );
    }

    #[test]
    fn every_kept_line_has_the_network_removed() {
        let (mut peer, _) = console(
            &[
                "@DUMP begin",
                "I (1) wifi: connected with oer-load-0123, key oer-4567",
                "@DUMP end error=0x0",
                "@OK DUMP",
            ],
            &["oer-load-0123", "oer-4567"],
        );
        assert_eq!(
            peer.dump().unwrap(),
            [
                "I (1) wifi: connected with <redacted>, key <redacted>",
                "@DUMP end error=0x0"
            ]
        );
    }

    #[test]
    fn the_ready_peer_is_this_receiver_with_its_address() {
        let (mut peer, _) = console(
            &[
                "ESP-ROM:esp32s31",
                "@READY protocol=2 target=esp32s31 app=wifi-udp-rx variant=fast",
                "@ADDR 02:00:00:00:00:01",
            ],
            &[],
        );
        let ready = peer.ready(Variant::Fast).unwrap();
        assert_eq!(ready[1].positional(0), Some("02:00:00:00:00:01"));
        for other in [
            "@READY protocol=2 target=esp32s31",
            "@READY protocol=1 target=esp32s31 app=wifi-udp-rx variant=fast",
            "@READY protocol=2 target=esp32s31 app=wifi-udp-rx variant=socket",
        ] {
            let (mut peer, _) = console(&[other, "@ADDR 02:00:00:00:00:01"], &[]);
            assert!(
                peer.ready(Variant::Fast)
                    .unwrap_err()
                    .contains("another peer"),
                "{other}"
            );
        }
    }

    #[test]
    fn report_fields_keep_numbers_as_numbers() {
        let Some(Line::Report(report)) =
            peer_line::parse("@LINK channel=13 second=2 rssi=-41 state=up down")
        else {
            panic!("a report line");
        };
        assert_eq!(
            fields(&report),
            json!({
                "channel": 13,
                "second": 2,
                "rssi": -41,
                "state": "up",
                "positional": ["down"],
            })
        );
    }

    /// A whole report of run 7, each line as the peer prints it.
    const REPORT: [&str; 9] = [
        "@RX run=7 datagrams=40 bytes=58880 window_datagrams=39 window_bytes=57408 lost=2 reordered=0 duplicates=0 untracked=0 highest=41 foreign=0 echoed=5 echo_refused=0 receive_errors=0 first_us=300100 last_us=5299000 window_us=5000000",
        "@SPAN run=7 window_from_us=300000 window_to_us=5300000 run_to_us=5800000",
        "@LWIP run=7 span=window link_recv=0 link_drop=0 ip_recv=44 ip_drop=0 udp_recv=44 udp_drop=0 udp_memerr=0",
        "@LWIP run=7 span=run link_recv=0 link_drop=0 ip_recv=45 ip_drop=0 udp_recv=45 udp_drop=0 udp_memerr=0",
        "@WIFIRX run=7 span=window tid=0 ht=44 ht_noeb=0 ht_retry=1",
        "@WIFIRX run=7 span=run tid=0 ht=45 ht_noeb=0 ht_retry=1",
        "@HWRX run=7 span=window rx_fcs_err=3 rx_mpdu=50 rx_buf_fullcnt=0 rx_fifo_ovfcnt=0",
        "@LINK run=7 channel=13 second=2 rssi=-30 disconnects=0",
        "@OK REPORT",
    ];

    fn parsed(lines: &[&str]) -> Vec<peer_line::Report> {
        lines
            .iter()
            .filter_map(|line| match peer_line::parse(line) {
                Some(Line::Report(report)) => Some(report),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_whole_report_of_the_run_is_taken() {
        let reports = parsed(&REPORT);
        let reported = Reported::of(&reports, 7).unwrap();
        assert_eq!(required::<u64>(reported.rx, "datagrams").unwrap(), 40);
        assert_eq!(reported.wifi_rx_run.field("ht"), Some("45"));
        assert_eq!(reported.lwip_window.field("udp_recv"), Some("44"));
    }

    #[test]
    fn a_report_of_another_run_is_an_error() {
        let stale = REPORT.map(|line| line.replace("run=7", "run=6"));
        let stale = stale.iter().map(String::as_str).collect::<Vec<_>>();
        let reports = parsed(&stale);
        let error = Reported::of(&reports, 7).err().unwrap();
        assert!(error.contains("of run 6, not 7"), "{error}");
        // One stale line among the run's own is enough.
        let mut mixed = REPORT.to_vec();
        let stale_span = "@SPAN run=6 window_from_us=1 window_to_us=2 run_to_us=3";
        mixed[1] = stale_span;
        let error = Reported::of(&parsed(&mixed), 7).err().unwrap();
        assert!(error.contains("SPAN line is of run 6"), "{error}");
    }

    #[test]
    fn a_missing_or_broken_mandatory_field_is_an_error_not_a_zero() {
        for (line, missing) in [
            (0, "datagrams=40 "),
            (0, "window_us=5000000"),
            (4, "ht_noeb=0 "),
            (3, "udp_recv=45 "),
            (6, "rx_fcs_err=3 "),
        ] {
            let mut lines = REPORT.map(str::to_owned);
            lines[line] = lines[line].replace(missing, "");
            let lines = lines.iter().map(String::as_str).collect::<Vec<_>>();
            let error = Reported::of(&parsed(&lines), 7).err();
            assert!(error.is_some(), "without `{missing}`");
        }
        let mut lines = REPORT.map(str::to_owned);
        lines[0] = lines[0].replace("datagrams=40", "datagrams=4x");
        let lines = lines.iter().map(String::as_str).collect::<Vec<_>>();
        assert!(Reported::of(&parsed(&lines), 7).is_err());
        // A line the peer could not fill is an error too.
        let mut lines = REPORT.to_vec();
        lines[5] = "@WIFIRX run=7 span=run error=0x102";
        assert!(
            Reported::of(&parsed(&lines), 7)
                .err()
                .unwrap()
                .contains("error 0x102")
        );
        // A missing span is missing even when the other span is there.
        let mut lines = REPORT.to_vec();
        lines.remove(3);
        assert!(
            Reported::of(&parsed(&lines), 7)
                .err()
                .unwrap()
                .contains("LWIP span=run")
        );
    }

    #[test]
    fn the_balance_names_what_the_access_point_sent_and_the_driver_did_not_count() {
        let reports = parsed(&REPORT);
        let reported = Reported::of(&reports, 7).unwrap();
        let router = json!({"to_device": {"packets": 50, "failed": 1}});
        let found = balance(&router, &reported).unwrap();
        assert_eq!(found["ap_minus_driver"], 5);
        assert_eq!(found["unclosed"], 5);
        let closed = balance(&json!({"to_device": {"packets": 45}}), &reported).unwrap();
        assert_eq!(closed["unclosed"], Value::Null);
        let unavailable = balance(&json!({"unavailable": "no router"}), &reported).unwrap();
        assert_eq!(unavailable["ap_packets"], Value::Null);
    }

    fn arguments(line: &[&str]) -> Result<Arguments, clap::Error> {
        <Arguments as clap::Parser>::try_parse_from(
            std::iter::once("wifi-load").chain(line.iter().copied()),
        )
    }

    #[test]
    fn the_command_line_selects_the_cases_and_the_board_before_the_stand() {
        let parsed = arguments(&["--board", "s31-b", "--case", "path"]).unwrap();
        assert_eq!(parsed.board.as_deref(), Some("s31-b"));
        assert!(!select(&parsed.cases).unwrap(), "path alone runs no level");
        assert!(select(&[]).unwrap());
        assert!(select(&["rx".to_owned()]).unwrap());
        assert!(
            select(&["stop".to_owned()])
                .unwrap_err()
                .contains("no case `stop`")
        );
        assert!(arguments(&["--list"]).unwrap().list);
        assert!(arguments(&["--bogus"]).is_err());
    }

    #[test]
    fn the_variant_is_socket_or_fast_and_names_its_image() {
        assert_eq!(parse_variant("socket").unwrap().image(), "wifi-udp-rx-peer");
        assert_eq!(
            parse_variant(" fast ").unwrap().image(),
            "wifi-udp-rx-fast-peer"
        );
        assert!(parse_variant("s31-b").is_err());
    }
}
