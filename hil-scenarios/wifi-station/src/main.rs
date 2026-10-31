//! The HIL Wi-Fi station scenario: the image's product station joins the
//! lab router's isolated WPA2-Personal network on a channel the stand file
//! allows, takes its IPv4 address by DHCP, answers the router's pings and
//! leaves. Every claim the device makes is checked against what the router
//! sees: its station table, its DHCP lease and its ping.
#![forbid(unsafe_code)]

use std::{
    net::Ipv4Addr,
    process::ExitCode,
    time::{Duration, Instant},
};

use oer_hil_agent::base::Service as _;
use oer_hil_harness::{Case, Cx, Failure, RouterOperation, Scenario, check};
use oer_services_wifi_station::{GetStatus, Join, Joined, Leave, Left, Status};

const SCENARIO: Scenario = Scenario::new(
    "wifi-station",
    "esp32s31-wifi-station",
    &[oer_services_wifi_station::Request::KEYS],
)
.using_router();

const CASES: [Case; 4] = [
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
        name: "leave",
        run: leave,
    },
];

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

fn main() -> ExitCode {
    oer_hil_harness::main(&SCENARIO, &CASES)
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
    let stand = oer_lab_model::path()
        .and_then(|path| oer_lab_model::Stand::load(&path))
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
    })?;
    let request = Join {
        ssid: heapless::String::try_from(ssid.as_str())
            .map_err(|_| Failure::Incomplete("the generated SSID is too long".into()))?,
        passphrase: heapless::String::try_from(passphrase.as_str())
            .map_err(|_| Failure::Incomplete("the generated passphrase is too long".into()))?,
        channel,
    };
    let answer = cx.call(request, JOIN_TIMEOUT)?;
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

/// The device takes an address by DHCP: the router leased it that address,
/// and the router is the gateway.
fn dhcp(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let started = Instant::now();
    let (address, ipv4) = loop {
        let status = status(cx)?;
        if let Some(ipv4) = status.ipv4 {
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

/// The device leaves: its link is down and the router's station table no
/// longer has it.
fn leave(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let station = mac(status(cx)?.address);
    let answer = cx.call(Leave, LEAVE_TIMEOUT)?;
    check(
        answer == Left::Down,
        format!("the leave answered {answer:?}"),
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
        assert!(ssid.len() <= oer_services_wifi_station::MAX_SSID, "{ssid}");
        assert!(
            (8..=oer_services_wifi_station::MAX_PASSPHRASE).contains(&passphrase.len()),
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
