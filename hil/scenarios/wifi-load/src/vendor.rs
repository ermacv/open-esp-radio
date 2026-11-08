//! The RX levels against the vendor receiver: the stand's ESP-IDF UDP
//! receiver peer (`tools/lab/peers/esp32s31-wifi-udp-rx`, catalog image
//! [`IMAGE`]) on a board of the stand instead of the image, so the vendor
//! driver and the open station take the same load through the same access
//! point. `OER_WIFI_LOAD_VENDOR=<board>` runs this instead of the cases.
//!
//! Under one lease of the board, the router and the air exclusively (the
//! cases' claims), the run writes the built peer unless the board runs it,
//! opens its console with a reset, applies the router's station profile
//! with a generated network, hands the peer that network in one `JOIN` line
//! and runs each RX level: the access point's view before and after it, the
//! host's load and probes as in [`crate::level`], and the peer's counts by
//! the load header with the vendor's own receive statistics. The generated
//! network's name and passphrase stay in memory: they go to the router
//! and to the peer's console input only, and every line kept of the peer's
//! console has them removed. Results go to
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

use oer_tools_hil_harness::{Failure, RouterOperation};
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

/// Names the board the vendor receiver runs on.
pub const BOARD_ENV: &str = "OER_WIFI_LOAD_VENDOR";
/// The peer's catalog image.
pub const IMAGE: &str = "wifi-udp-rx-peer";
/// How long the peer's boot may take to say `@READY` after the reset.
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// The peer's own join wait is 30 s.
const JOIN_TIMEOUT: Duration = Duration::from_secs(40);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// The board `OER_WIFI_LOAD_VENDOR` names, when it names one.
pub fn board() -> Option<String> {
    std::env::var(BOARD_ENV)
        .ok()
        .map(|board| board.trim().to_owned())
        .filter(|board| !board.is_empty())
}

/// Run the RX levels against the vendor receiver on `board`: 0 when every
/// level ran, 2 when the run could not finish.
pub fn main(board: &str) -> ExitCode {
    let mut measurements = Vec::new();
    let ended = run(board, &mut measurements);
    let verdict = match &ended {
        Ok(()) => "pass",
        Err(reason) => {
            eprintln!("vendor: {reason}");
            "incomplete"
        }
    };
    if !measurements.is_empty() {
        write_results(board, &measurements, &ended, verdict);
    }
    if ended.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}

fn run(selector: &str, measurements: &mut Vec<(String, Value)>) -> Result<(), String> {
    let root = oer_tools_os_process::Checkout::discover("HIL scenario")
        .map_err(|error| error.to_string())?
        .root;
    let bundle = bundle(&root)?;
    let stand = crate::stand().map_err(failure)?;
    let board = stand.resolve(selector).map_err(|error| error.to_string())?;
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
            work: format!("hil wifi-load vendor {IMAGE}"),
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
    let ran = leased(&root, &mut client, &mac, &owner, &bundle, measurements);
    let released = client.release();
    ran.and(released)
}

/// The peer's image bundle, from its current catalog build.
fn bundle(root: &std::path::Path) -> Result<std::path::PathBuf, String> {
    use oer_tools_image_build::esp_idf::catalog;
    let build = catalog::current(root, IMAGE).map_err(text)?;
    let entries = catalog::entries(root).map_err(text)?;
    let entry = catalog::entry(&entries, IMAGE).map_err(text)?;
    let output = oer_tools_image_esp_idf::output(root, &entry.project(root)).join("bundle");
    let bundle = catalog::bundle(root, entry, &build, &output).map_err(text)?;
    eprintln!(
        "vendor: {IMAGE} {} ({})",
        build.application_sha256,
        bundle.directory().display()
    );
    Ok(bundle.directory().to_owned())
}

fn text(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}

fn failure(failure: Failure) -> String {
    match failure {
        Failure::Fail(reason) | Failure::Incomplete(reason) => reason,
    }
}

fn leased(
    root: &std::path::Path,
    client: &mut Client,
    mac: &str,
    owner: &str,
    bundle: &std::path::Path,
    measurements: &mut Vec<(String, Value)>,
) -> Result<(), String> {
    let git = |arguments: &[&str]| oer_tools_os_process::git::text(root, arguments).ok();
    let written = client.run(
        Operation::Flash {
            board: mac.to_owned(),
            bundle: bundle.display().to_string(),
            image: ImageIdentity {
                name: IMAGE.to_owned(),
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
    let ready = peer.ready()?;
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
            width: width().map_err(failure)?,
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
            "image": IMAGE,
            "host": host.to_string(),
            "device": device.to_string(),
            "joined": fields(joined),
        }),
    );
    let probing = probes().map_err(failure)?;
    for total in levels_mbps().map_err(failure)? {
        let offer = Offer::level(Mode::Rx, total, probing).map_err(failure)?;
        level(
            client,
            &mut peer,
            &address,
            device,
            host,
            &offer,
            measurements,
        )?;
    }
    peer.close();
    Ok(())
}

/// One RX level against the peer, as [`crate::level`] runs it against the
/// image.
fn level(
    client: &mut Client,
    peer: &mut Peer,
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
    let window = window_ms().map_err(failure)?;
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
        let probes = probes.map_or(0, |probes| probes.join().unwrap_or(0));
        std::thread::sleep(Duration::from_millis(u64::from(DRAIN_MS)));
        (sent, probes)
    });
    let after = read(client);
    let span = since.elapsed();
    let reports = peer.command(&format!("REPORT {run}"), COMMAND_TIMEOUT)?;
    let dump = peer.dump()?;
    let mut received = endpoint.close();
    received.echoes.sent = probes;
    let report = |name: &str| reports.iter().find(|report| report.name == name);
    let rx = report("RX").ok_or("the peer's report holds no RX line")?;
    let number = |key: &str| rx.parsed::<u64>(key).unwrap_or(0);
    let window_s = rx.parsed::<u64>("window_us").unwrap_or(0) as f64 / 1e6;
    let datagrams = number("datagrams");
    let value = json!({
        "target": "vendor",
        "image": IMAGE,
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
            "device_received_mbps": mbps(number("window_bytes"), window_s),
            "device_received_datagrams": datagrams,
            "device_received_in_window": number("window_datagrams"),
            "lost": sent.datagrams.saturating_sub(datagrams),
            "lost_percent": percent(sent.datagrams.saturating_sub(datagrams), sent.datagrams),
            "lost_below_highest": number("lost"),
            "reordered": number("reordered"),
            "duplicates": number("duplicates"),
            "untracked": number("untracked"),
            "foreign": number("foreign"),
        },
        "link": { "router": link::router_window(&before, &after, span) },
        "vendor": {
            "rx": fields(rx),
            "lwip": report("LWIP").map(fields),
            "wifi_rx": report("WIFIRX").map(fields),
            "hw_rx": report("HWRX").map(fields),
            "link": report("LINK").map(fields),
            "dump": dump,
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
        "image": IMAGE,
        "board": board,
        "unix": nanos / 1_000_000_000,
        "reason": ended.as_ref().err(),
        "verdict": verdict,
        "measurements": measurements.iter().map(|(name, value)| json!({
            "case": "rx",
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

    /// The reports up to the boot's `@READY` of this peer.
    fn ready(&mut self) -> Result<Vec<peer_line::Report>, String> {
        let deadline = Instant::now() + READY_TIMEOUT;
        let mut ready = None;
        while let Some(line) = self.next(deadline) {
            match peer_line::parse(&line) {
                Some(Line::Ready { protocol, report }) => {
                    if protocol != 1 || report.field("app") != Some("wifi-udp-rx") {
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
                "@READY protocol=1 target=esp32s31 app=wifi-udp-rx",
                "@ADDR 02:00:00:00:00:01",
            ],
            &[],
        );
        let ready = peer.ready().unwrap();
        assert_eq!(ready[1].positional(0), Some("02:00:00:00:00:01"));
        let (mut peer, _) = console(
            &[
                "@READY protocol=1 target=esp32s31",
                "@ADDR 02:00:00:00:00:01",
            ],
            &[],
        );
        assert!(peer.ready().unwrap_err().contains("another peer"));
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
}
