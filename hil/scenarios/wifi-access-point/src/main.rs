//! The HIL Wi-Fi access-point scenario: the image's access point serves a
//! generated WPA2-Personal BSS on a channel the stand file allows, beacons
//! once per TBTT, takes the stand's Linux station (the `linux-net`
//! fixture's `wlan0`) through association and the 4-way handshake, answers
//! its pings at its static address and stops. Every claim the device makes
//! is checked against what the station sees: its supplicant's state and its
//! ping.
#![forbid(unsafe_code)]

use std::{
    io::Write as _,
    process::{Command, ExitCode, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use oer_hil_services_wifi_access_point::{
    AccessPointError, GetStatus, Ipv4, Serve, Served, Status, Stop, Stopped,
};
use oer_protocols_hil_agent::base::Service as _;
use oer_tools_hil_harness::{Case, Cx, Failure, Scenario, check};

/// The station's air is the stand's: no other lease may use it meanwhile.
const SCENARIO: Scenario = Scenario::new(
    "wifi-access-point",
    "hil-wifi-access-point",
    &[oer_hil_services_wifi_access_point::Request::KEYS],
)
.using_air_exclusively();

const CASES: [Case; 5] = [
    Case {
        name: "serve",
        run: serve,
    },
    Case {
        name: "beacons",
        run: beacons,
    },
    Case {
        name: "associate",
        run: associate,
    },
    Case {
        name: "ping",
        run: ping,
    },
    Case {
        name: "stop",
        run: stop,
    },
];

/// The PMK derivation, the radio's start and the first beacon, with the
/// image's own bound of 20 s inside.
const SERVE_TIMEOUT: Duration = Duration::from_secs(25);
const STATUS_TIMEOUT: Duration = Duration::from_secs(2);
/// The image's stop bound of 10 s inside.
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
/// The window the beacon count is read over.
const BEACON_WINDOW: Duration = Duration::from_secs(3);
/// The image's beacon interval, 100 TU (`ApSettings::wpa2`).
const BEACON_INTERVAL: Duration = Duration::from_micros(102_400);
/// Scanning, authentication, association and the 4-way handshake.
const ASSOCIATE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the device may take to count what the station did.
const DEVICE_SEES: Duration = Duration::from_secs(5);
/// The access point's static address (the image's README).
const ACCESS_POINT_ADDRESS: [u8; 4] = [192, 168, 4, 1];
/// The station's static address on the access point's network.
const STATION_CIDR: &str = "192.168.4.2/24";
/// Pings the station sends and the replies the case needs.
const PINGS: u32 = 20;
const REPLIES: u32 = 19;
/// The fixture's station program and its private supplicant socket
/// (`tools/lab/linux-net/README.md`).
const FIXTURE: &str = "/usr/local/sbin/open-radio-net";
const SUPPLICANT: &str = "/run/open-radio-wpa-control";

/// The run's network, generated once.
static NETWORK: std::sync::OnceLock<(String, String, u8)> = std::sync::OnceLock::new();
/// Whether the fixture's station was prepared: the run returns it.
static STATION: AtomicBool = AtomicBool::new(false);

fn main() -> ExitCode {
    let code = oer_tools_hil_harness::main(&SCENARIO, &CASES);
    if STATION.load(Ordering::SeqCst)
        && let Err(error) = release_station()
    {
        eprintln!("error: the fixture's station was not returned: {error}");
        return ExitCode::FAILURE;
    }
    code
}

/// The network the run generates: its SSID and passphrase.
fn generated() -> (String, String) {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
        ^ u128::from(std::process::id());
    let tag = format!("{seed:032x}");
    (
        format!("oer-ap-{}", tag.get(24..).unwrap_or("0")),
        format!("oer-{}", tag.get(8..).unwrap_or("0")),
    )
}

/// The channel the stand file's router allows first.
fn channel() -> Result<u8, Failure> {
    let stand = oer_tools_lab_model::path()
        .and_then(|path| oer_tools_lab_model::Stand::load(&path))
        .map_err(|error| Failure::Incomplete(format!("the stand file: {error}")))?;
    stand
        .router
        .as_ref()
        .and_then(|router| router.channels.first().copied())
        .ok_or_else(|| Failure::Incomplete("the stand file's [router] allows no channel".into()))
}

fn network() -> Result<&'static (String, String, u8), Failure> {
    if let Some(network) = NETWORK.get() {
        return Ok(network);
    }
    let (ssid, passphrase) = generated();
    let channel = channel()?;
    Ok(NETWORK.get_or_init(|| (ssid, passphrase, channel)))
}

fn status(cx: &mut Cx<'_>) -> Result<Status, Failure> {
    cx.call(GetStatus, STATUS_TIMEOUT)
}

/// The device's status until `seen` accepts it, up to `within`; the last
/// one either way.
fn status_until(
    cx: &mut Cx<'_>,
    seen: impl Fn(&Status) -> bool,
    within: Duration,
) -> Result<(bool, Status), Failure> {
    let started = Instant::now();
    loop {
        let status = status(cx)?;
        if seen(&status) || started.elapsed() >= within {
            return Ok((seen(&status), status));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn mac(address: [u8; 6]) -> String {
    address
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The device serves the run's BSS: it beacons at its own address, with
/// its static IPv4 configuration.
fn serve(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let (ssid, passphrase, channel) = network()?;
    let request = Serve {
        ssid: heapless::String::try_from(ssid.as_str())
            .map_err(|_| Failure::Incomplete("the SSID fits the service".into()))?,
        passphrase: heapless::String::try_from(passphrase.as_str())
            .map_err(|_| Failure::Incomplete("the passphrase fits the service".into()))?,
        channel: *channel,
        clients: 2,
    };
    let address = match cx.call(request, SERVE_TIMEOUT)? {
        Served::Beaconing { address } => address,
        Served::Refused(AccessPointError::Unsupported) => {
            return Err(Failure::Fail(
                "serve refused: the radio cannot serve an access point (its port declares no \
                 beacon Timestamp rule); refused before the radio started"
                    .into(),
            ));
        }
        Served::Refused(error) => return Err(Failure::Fail(format!("serve refused: {error:?}"))),
    };
    cx.measure("bssid", serde_json::json!(mac(address)));
    cx.measure("channel", serde_json::json!(channel));
    let status = status(cx)?;
    check(status.running, "the access point runs")?;
    check(status.address == address, "the status names the BSSID")?;
    check(
        status.ipv4
            == Some(Ipv4 {
                address: ACCESS_POINT_ADDRESS,
                prefix: 24,
            }),
        format!("the static IPv4 configuration, not {:?}", status.ipv4),
    )
}

/// Every TBTT of a window ends one beacon attempt: delivered or expired,
/// their count is the window's TBTTs, within the two status requests'
/// own latency of one interval each.
fn beacons(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let before = status(cx)?;
    let started = Instant::now();
    std::thread::sleep(BEACON_WINDOW);
    let after = status(cx)?;
    let window = started.elapsed();
    let delivered = after.beacons.saturating_sub(before.beacons);
    let expired = after.beacons_expired.saturating_sub(before.beacons_expired);
    let tbtts = window.as_micros() / BEACON_INTERVAL.as_micros();
    cx.measure(
        "beacons",
        serde_json::json!({
            "window_ms": window.as_millis(),
            "tbtts": tbtts,
            "delivered": delivered,
            "expired": expired,
        }),
    );
    let attempts = u128::from(delivered) + u128::from(expired);
    check(
        attempts.abs_diff(tbtts) <= 2,
        format!("{attempts} beacon attempts in {tbtts} TBTTs"),
    )?;
    check(expired == 0, format!("{expired} beacons missed their TBTT"))
}

/// The fixture's station associates to the BSS and completes the 4-way
/// handshake; the device counts it associated and authorized.
fn associate(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let (ssid, passphrase, channel) = network()?;
    let frequency = 2407 + 5 * u16::from(*channel);
    let profile = serde_json::json!({
        "ssid_hex": ssid.bytes().map(|byte| format!("{byte:02x}")).collect::<String>(),
        "security": "wpa2",
        "passphrase": passphrase,
        "frequency_mhz": frequency,
        "phy": "ht",
        "ipv4": {"mode": "static", "address": STATION_CIDR},
    });
    STATION.store(true, Ordering::SeqCst);
    // The passphrase goes over the fixture's stdin only.
    let mut client = Command::new("sudo")
        .args(["-n", FIXTURE, "client"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|error| Failure::Incomplete(format!("the fixture's client: {error}")))?;
    let written = client
        .stdin
        .take()
        .ok_or_else(|| Failure::Incomplete("the fixture's stdin".into()))
        .and_then(|mut stdin| {
            stdin
                .write_all(profile.to_string().as_bytes())
                .map_err(|error| Failure::Incomplete(format!("the profile: {error}")))
        });
    let exited = client
        .wait()
        .map_err(|error| Failure::Incomplete(format!("the fixture's client: {error}")))?;
    written?;
    check_incomplete(exited.success(), "the fixture prepared its station")?;
    let enabled = supplicant(&["enable_network", "0"])?;
    check_incomplete(enabled.trim() == "OK", format!("enable_network: {enabled}"))?;
    let started = Instant::now();
    let state = loop {
        let state = supplicant(&["status"])?;
        if state.lines().any(|line| line == "wpa_state=COMPLETED")
            || started.elapsed() >= ASSOCIATE_TIMEOUT
        {
            break state;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let station_state = state
        .lines()
        .find_map(|line| line.strip_prefix("wpa_state="))
        .unwrap_or("unknown")
        .to_owned();
    cx.measure(
        "associate",
        serde_json::json!({
            "ms": started.elapsed().as_millis(),
            "station_state": station_state,
        }),
    );
    check(
        station_state == "COMPLETED",
        format!("the station's supplicant ends in {station_state}"),
    )?;
    let bssid = state
        .lines()
        .find_map(|line| line.strip_prefix("bssid="))
        .unwrap_or("")
        .to_owned();
    let (seen, status) = status_until(
        cx,
        |status| status.associated == 1 && status.authorized == 1,
        DEVICE_SEES,
    )?;
    check(
        bssid == mac(status.address),
        format!("the station joined {bssid}, not the device's BSS"),
    )?;
    check(
        seen,
        format!(
            "the device counts {} associated, {} authorized",
            status.associated, status.authorized
        ),
    )
}

/// The station pings the access point's static address: each request goes
/// up through the BSS and each reply comes down.
fn ping(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let target = std::net::Ipv4Addr::from(ACCESS_POINT_ADDRESS).to_string();
    let count = PINGS.to_string();
    let output = Command::new("ping")
        .args(["-I", "wlan0", "-c", &count, "-i", "0.2", "-W", "1", &target])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| Failure::Incomplete(format!("ping: {error}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let received = text
        .lines()
        .find(|line| line.contains("packets transmitted"))
        .and_then(|line| line.split(", ").nth(1))
        .and_then(|part| part.split(' ').next())
        .and_then(|number| number.parse::<u32>().ok())
        .ok_or_else(|| Failure::Incomplete(format!("ping's summary: {text}")))?;
    let status = status(cx)?;
    cx.measure(
        "ping",
        serde_json::json!({
            "sent": PINGS,
            "received": received,
            "rx_dropped": status.rx_dropped,
            "refused": status.refused,
        }),
    );
    check(
        received >= REPLIES,
        format!("{received} of {PINGS} pings answered"),
    )
}

/// The device stops: it no longer beacons, and its station leaves.
fn stop(cx: &mut Cx<'_>) -> Result<(), Failure> {
    match cx.call(Stop, STOP_TIMEOUT)? {
        Stopped::Down => {}
        Stopped::Ended(end) => {
            return Err(Failure::Fail(format!(
                "the access point ended otherwise: {end}"
            )));
        }
        Stopped::Refused(error) => return Err(Failure::Fail(format!("stop refused: {error:?}"))),
    }
    let status = status(cx)?;
    check(!status.running, "the access point stopped")?;
    check(
        status.associated == 0,
        format!("{} stations stay associated", status.associated),
    )
}

fn check_incomplete(condition: bool, what: impl Into<String>) -> Result<(), Failure> {
    if condition {
        Ok(())
    } else {
        Err(Failure::Incomplete(what.into()))
    }
}

/// One command of the fixture's private supplicant: its output.
fn supplicant(arguments: &[&str]) -> Result<String, Failure> {
    let output = Command::new("wpa_cli")
        .args(["-p", SUPPLICANT, "-i", "wlan0"])
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| Failure::Incomplete(format!("wpa_cli: {error}")))?;
    check_incomplete(
        output.status.success(),
        format!("wpa_cli {arguments:?} exited {}", output.status),
    )?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Stop the fixture's station and return `wlan0` to its managed state.
fn release_station() -> Result<(), String> {
    for operation in ["stop", "managed"] {
        let exited = Command::new("sudo")
            .args(["-n", FIXTURE, operation])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .map_err(|error| format!("{operation}: {error}"))?;
        if !exited.success() {
            return Err(format!("{operation} exited {exited}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_network_fits_the_service_and_differs_by_run() {
        let (ssid, passphrase) = generated();
        assert!(
            ssid.len() <= oer_hil_services_wifi_access_point::MAX_SSID,
            "{ssid}"
        );
        assert!(
            (8..=oer_hil_services_wifi_access_point::MAX_PASSPHRASE).contains(&passphrase.len()),
            "{passphrase}"
        );
        std::thread::sleep(Duration::from_millis(1));
        assert_ne!(generated().0, ssid);
    }
}
