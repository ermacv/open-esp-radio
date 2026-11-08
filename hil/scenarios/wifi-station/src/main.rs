//! The HIL Wi-Fi station scenario: the image's product station joins the
//! lab router's isolated WPA2-Personal network on a channel the stand file
//! allows, takes its IPv4 address by DHCP, answers the router's pings,
//! restarts after a refused publication with the chip still running, and
//! leaves. Every claim the device makes is checked against what the router
//! sees: its station table, its DHCP lease and its ping.
#![forbid(unsafe_code)]

use std::{
    net::Ipv4Addr,
    process::ExitCode,
    time::{Duration, Instant},
};

use oer_hil_services_fault::{GetRestarts, RefusePublication, Restarts};
use oer_hil_services_wifi_station::{
    GetConnectTimes, GetScanVisit, GetStatus, Join, Joined, Leave, Left, ScanVisit,
    ScanVisitAnswer, Status,
};
use oer_protocols_hil_agent::base::Service as _;
use oer_tools_hil_harness::{
    Case, Cx, Failure, RouterChannelWidth, RouterOperation, Scenario, check,
};

const SCENARIO: Scenario = Scenario::new(
    "wifi-station",
    "hil-wifi-station",
    &[
        oer_hil_services_wifi_station::Request::KEYS,
        oer_hil_services_fault::Request::KEYS,
    ],
)
.using_router();

const CASES: [Case; 5] = [
    Case {
        name: "join",
        run: join,
    },
    Case {
        name: "dhcp",
        run: dhcp,
    },
    Case {
        name: "ping",
        run: ping,
    },
    Case {
        name: "restart",
        run: restart,
    },
    Case {
        name: "leave",
        run: leave,
    },
];

/// When the run's join was sent.
static JOIN_STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Scanning, authentication, association and the 4-way handshake.
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the router may take to show what the device did.
const ROUTER_SEES: Duration = Duration::from_secs(10);
/// DHCP discovery to a bound lease.
const DHCP_TIMEOUT: Duration = Duration::from_secs(30);
const STATUS_TIMEOUT: Duration = Duration::from_secs(2);
const LEAVE_TIMEOUT: Duration = Duration::from_secs(10);
/// The router's address on the isolated station network
/// (`tools/lab/router/README.md`), the lease's gateway.
const ROUTER_ADDRESS: [u8; 4] = [10, 231, 0, 1];
/// Pings the router sends and the replies the case needs.
const PINGS: u16 = 10;
const REPLIES: u64 = 9;
/// The console line of the injected refusal: a publication refused for its key.
const INJECTED_CAUSE: &str = "wifi-restarted Port/Unpublished/Key";
/// A stop and a new installation.
const RESTART_TIMEOUT: Duration = Duration::from_secs(10);

fn main() -> ExitCode {
    oer_tools_hil_harness::main(&SCENARIO, &CASES)
}

/// The network the run generates: its SSID and passphrase.
fn network() -> (String, String) {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
        ^ u128::from(std::process::id());
    let tag = format!("{seed:032x}");
    (
        format!("oer-sta-{}", tag.get(24..).unwrap_or("0")),
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

fn mac(address: [u8; 6]) -> String {
    address
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn status(cx: &mut Cx<'_>) -> Result<Status, Failure> {
    cx.call(GetStatus, STATUS_TIMEOUT)
}

/// Ask the router until `seen` accepts its answer to `request`, up to
/// `within`; the last answer either way.
fn router_until(
    cx: &Cx<'_>,
    request: impl Fn() -> RouterOperation,
    seen: impl Fn(&serde_json::Value) -> bool,
    within: Duration,
) -> Result<(bool, serde_json::Value), Failure> {
    let started = Instant::now();
    loop {
        let answer = cx.router(request())?;
        if seen(&answer) || started.elapsed() >= within {
            return Ok((seen(&answer), answer));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn joined_by_router(station: &serde_json::Value) -> bool {
    ["present", "authenticated", "associated", "authorized"]
        .iter()
        .all(|field| station.get(field) == Some(&serde_json::Value::Bool(true)))
}

/// The router brings up a generated WPA2 network; the device joins it, and
/// the router's station table shows it authorized.
fn join(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let channel = channel()?;
    let (ssid, passphrase) = network();
    cx.router(RouterOperation::PrepareNetwork)?;
    cx.router(RouterOperation::ApplyStationProfile {
        ssid: ssid.clone(),
        passphrase: passphrase.clone(),
        channel,
        width: RouterChannelWidth::Ht20,
        forward_host: None,
    })?;
    let request = Join {
        ssid: heapless::String::try_from(ssid.as_str())
            .map_err(|_| Failure::Incomplete("the generated SSID is too long".into()))?,
        passphrase: heapless::String::try_from(passphrase.as_str())
            .map_err(|_| Failure::Incomplete("the generated passphrase is too long".into()))?,
        channel,
        he20: false,
    };
    let _ = JOIN_STARTED.set(Instant::now());
    let answer = cx.call(request, JOIN_TIMEOUT)?;
    connect_report(cx)?;
    let Joined::Linked {
        channel: linked, ..
    } = answer
    else {
        return Err(Failure::Fail(format!("the join failed: {answer:?}")));
    };
    check(
        linked == channel,
        format!("linked on channel {linked}, the network is on {channel}"),
    )?;
    let station = mac(status(cx)?.address);
    let (joined, seen) = router_until(
        cx,
        || RouterOperation::Station {
            mac: station.clone(),
        },
        joined_by_router,
        ROUTER_SEES,
    )?;
    check(joined, format!("the router's station table shows {seen}"))
}

/// Print the connect's timeline, milliseconds from its start, and its scan
/// by channel.
fn connect_report(cx: &mut Cx<'_>) -> Result<(), Failure> {
    println!("connect: {}", cx.call(GetConnectTimes, STATUS_TIMEOUT)?);
    println!("connect: {}", ScanVisit::HEADER);
    let mut index = 0;
    while let ScanVisitAnswer(Some(visit)) = cx.call(GetScanVisit { index }, STATUS_TIMEOUT)? {
        println!("connect: {visit}");
        index += 1;
    }
    Ok(())
}

/// The device takes an address by DHCP: the router leased it that address,
/// and the router is the gateway.
fn dhcp(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let started = Instant::now();
    let (address, ipv4) = loop {
        let status = status(cx)?;
        if let Some(ipv4) = status.ipv4 {
            println!(
                "connect: address {} ms from the join's start",
                JOIN_STARTED
                    .get()
                    .map_or(started.elapsed(), Instant::elapsed)
                    .as_millis()
            );
            break (status.address, ipv4);
        }
        if started.elapsed() >= DHCP_TIMEOUT {
            return Err(Failure::Fail(format!(
                "no DHCP address within {DHCP_TIMEOUT:?}: {status:?}"
            )));
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let device = Ipv4Addr::from(ipv4.address).to_string();
    let lease = cx.router(RouterOperation::Lease { mac: mac(address) })?;
    check(
        lease.get("address").and_then(serde_json::Value::as_str) == Some(device.as_str()),
        format!("the device has {device}, the router's lease is {lease}"),
    )?;
    check(
        ipv4.gateway == Some(ROUTER_ADDRESS),
        format!("the gateway is {:?}, not the router", ipv4.gateway),
    )
}

/// The router pings the device's address; the device's IP stack answers.
fn ping(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let ipv4 = status(cx)?
        .ipv4
        .ok_or_else(|| Failure::Fail("the device has no address".into()))?;
    let answer = cx.router(RouterOperation::Ping {
        address: Ipv4Addr::from(ipv4.address),
        count: PINGS,
    })?;
    let received = answer
        .get("received")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    check(
        received >= REPLIES,
        format!("{received} of {PINGS} pings answered: {answer}"),
    )
}

/// A refused publication restarts the station, not the chip: the same boot
/// counts one restart, and the station reconnects by itself and carries data.
fn restart(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let (before, _) = restarts(cx)?;
    let ipv4 = status(cx)?
        .ipv4
        .ok_or_else(|| Failure::Fail("the device has no address".into()))?;
    cx.call(RefusePublication, STATUS_TIMEOUT)?;
    // Its answers are the publications the refusal stops.
    cx.router(RouterOperation::Ping {
        address: Ipv4Addr::from(ipv4.address),
        count: PINGS,
    })?;
    let started = Instant::now();
    let cause = loop {
        let (now, cause) = restarts(cx)?;
        if now == before + 1 {
            break cause;
        }
        if started.elapsed() >= RESTART_TIMEOUT {
            return Err(Failure::Fail(format!(
                "{now} restarts within {RESTART_TIMEOUT:?}, {before} before"
            )));
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    cx.measure("restart cause", serde_json::json!(cause));
    check(
        cause == INJECTED_CAUSE,
        format!("the restart's cause is `{cause}`, not the refused publication"),
    )?;
    // The station's reconnect policy brings it back with no command here.
    let started = Instant::now();
    loop {
        let status = status(cx)?;
        if status.link.is_some() && status.ipv4.is_some() {
            break;
        }
        if started.elapsed() >= JOIN_TIMEOUT + DHCP_TIMEOUT {
            return Err(Failure::Fail(format!(
                "not back on the network: {status:?}"
            )));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    ping(cx)
}

/// The restart count and the last cause.
fn restarts(cx: &mut Cx<'_>) -> Result<(u32, String), Failure> {
    let Restarts { count, cause } = cx.call(GetRestarts, STATUS_TIMEOUT)?;
    Ok((count, cause.to_string()))
}

/// The device leaves: its link is down and the router's station table no
/// longer has it.
fn leave(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let station = mac(status(cx)?.address);
    let answer = cx.call(Leave, LEAVE_TIMEOUT)?;
    check(
        answer == Left::Down,
        match answer {
            Left::DownNetworkLost => {
                "the leave stopped the radio, but the network's stop participant was lost".into()
            }
            answer => format!("the leave answered {answer:?}"),
        },
    )?;
    let after = status(cx)?;
    check(after.link.is_none(), format!("after leaving: {after:?}"))?;
    let absent = |station: &serde_json::Value| {
        station.get("present") == Some(&serde_json::Value::Bool(false))
    };
    let (gone, seen) = router_until(
        cx,
        || RouterOperation::Station {
            mac: station.clone(),
        },
        absent,
        ROUTER_SEES,
    )?;
    check(gone, format!("the router still lists the station: {seen}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_network_fits_the_service_and_differs_by_run() {
        let (ssid, passphrase) = network();
        assert!(
            ssid.len() <= oer_hil_services_wifi_station::MAX_SSID,
            "{ssid}"
        );
        assert!(
            (8..=oer_hil_services_wifi_station::MAX_PASSPHRASE).contains(&passphrase.len()),
            "{passphrase}"
        );
        std::thread::sleep(Duration::from_millis(1));
        assert_ne!(network().0, ssid);
    }

    #[test]
    fn the_router_shows_a_joined_station_only_with_all_four_flags() {
        let joined = serde_json::json!({
            "present": true, "authenticated": true, "associated": true, "authorized": true
        });
        assert!(joined_by_router(&joined));
        let mut half = joined.clone();
        half["authorized"] = serde_json::json!(false);
        assert!(!joined_by_router(&half));
        assert_eq!(
            mac([0x1c, 0x29, 0x04, 0xd0, 0x23, 0xa6]),
            "1c:29:04:d0:23:a6"
        );
    }
}
