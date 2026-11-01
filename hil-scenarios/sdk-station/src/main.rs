//! The HIL SDK station scenario: the `oer-espressif` station of the
//! `esp32s31-sdk-station` image meets the lab router's isolated WPA2
//! network. A wrong passphrase is a handled result without a reset; the
//! station joins, takes its DHCP address, exchanges a line over TCP with
//! this host, rejoins by itself after the router deauthenticates it, and a
//! wait for an address times out while the router serves no DHCP. Every
//! claim of the device is checked against the router or this host.
#![forbid(unsafe_code)]

use std::{
    io::{Read as _, Write as _},
    net::{Ipv4Addr, SocketAddr, TcpListener},
    process::ExitCode,
    sync::mpsc,
    time::{Duration, Instant},
};

use oer_hil_agent::base::Service as _;
use oer_hil_harness::{Case, Cx, Failure, RouterOperation, Scenario, check};
use oer_services_sdk_station::{
    Address, Connect, Connection, Exchange, Exchanged, GetStatus, Ipv4, LinkLoss, MAX_LINE, Status,
    WaitIpv4, WaitLinkDown,
};

const SCENARIO: Scenario = Scenario::new(
    "sdk-station",
    "esp32s31-sdk-station",
    &[oer_services_sdk_station::Request::KEYS],
)
.using_router();

const CASES: [Case; 6] = [
    Case {
        name: "wrong-passphrase",
        run: wrong_passphrase,
    },
    Case {
        name: "join",
        run: join,
    },
    Case {
        name: "dhcp",
        run: dhcp,
    },
    Case {
        name: "tcp",
        run: tcp,
    },
    Case {
        name: "reconnect",
        run: reconnect,
    },
    Case {
        name: "no-dhcp",
        run: no_dhcp,
    },
];

/// A scan of every channel the image's domain permits, authentication,
/// association and the 4-way handshake, or their failure.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
/// What the device's own waits are bounded by, and the call's margin.
const DHCP_WAIT_MS: u32 = 30_000;
const REJOIN_WAIT_MS: u32 = 60_000;
const LINK_WAIT_MS: u32 = 10_000;
const EXCHANGE_MS: u32 = 10_000;
/// How long a lease may take not to come while the router serves no DHCP.
const NO_DHCP_WAIT_MS: u32 = 20_000;
const MARGIN: Duration = Duration::from_secs(5);
/// How long the router may take to show what the device did.
const ROUTER_SEES: Duration = Duration::from_secs(10);
const STATUS_TIMEOUT: Duration = Duration::from_secs(2);
/// The router's address on the isolated station network
/// (`tools/lab/router/README.md`), the lease's gateway.
const ROUTER_ADDRESS: [u8; 4] = [10, 231, 0, 1];
const STATION_NETWORK: &str = "10.231.0.0/24";

fn main() -> ExitCode {
    oer_hil_harness::main(&SCENARIO, &CASES)
}

fn stand() -> Result<oer_lab_model::Stand, Failure> {
    oer_lab_model::path()
        .and_then(|path| oer_lab_model::Stand::load(&path))
        .map_err(|error| Failure::Incomplete(format!("the stand file: {error}")))
}

/// The channel the stand file's router allows first.
fn channel() -> Result<u8, Failure> {
    stand()?
        .router
        .as_ref()
        .and_then(|router| router.channels.first().copied())
        .ok_or_else(|| Failure::Incomplete("the stand file's [router] allows no channel".into()))
}

/// A value that differs by run.
fn tag() -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
        ^ u128::from(std::process::id());
    format!("{seed:032x}")
}

/// The network the run generates: its SSID and passphrase.
fn network() -> (String, String) {
    let tag = tag();
    (
        format!("oer-sdk-{}", tag.get(24..).unwrap_or("0")),
        format!("oer-{}", tag.get(8..).unwrap_or("0")),
    )
}

fn mac(address: [u8; 6]) -> String {
    address
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn milliseconds(ms: u32) -> Duration {
    Duration::from_millis(u64::from(ms))
}

fn status(cx: &mut Cx<'_>) -> Result<Status, Failure> {
    cx.call(GetStatus, STATUS_TIMEOUT)
}

/// The host's address toward the station network: it must leave through
/// the lab router the stand file names.
fn host_address() -> Result<Ipv4Addr, Failure> {
    let to = Ipv4Addr::from(ROUTER_ADDRESS);
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
    let source = field("src")
        .and_then(|source| source.parse().ok())
        .ok_or_else(|| Failure::Incomplete(format!("no source address toward {to}: `{line}`")))?;
    let gateway: Option<Ipv4Addr> = field("via").and_then(|via| via.parse().ok());
    let lan_address = stand()?
        .router
        .and_then(|router| router.lan_address)
        .ok_or_else(|| {
            Failure::Incomplete("the stand file's [router] names no lan_address".into())
        })?;
    if gateway != Some(lan_address) {
        return Err(Failure::Incomplete(format!(
            "the host does not reach {to} through the lab router {lan_address} \
             (`ip route add {STATION_NETWORK} via {lan_address}`, once): `{line}`"
        )));
    }
    Ok(source)
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

fn connect(ssid: &str, passphrase: &str) -> Result<Connect, Failure> {
    Ok(Connect {
        ssid: heapless::String::try_from(ssid)
            .map_err(|_| Failure::Incomplete("the generated SSID is too long".into()))?,
        passphrase: heapless::String::try_from(passphrase)
            .map_err(|_| Failure::Incomplete("the generated passphrase is too long".into()))?,
    })
}

/// The run's network: generated by the first case, used by the next.
static NETWORK: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);

/// The run's network, applied on the router by the first case that needs
/// it: a generated WPA2 network forwarding TCP and UDP to this host.
fn the_network(cx: &mut Cx<'_>) -> Result<(String, String), Failure> {
    if let Some(network) = NETWORK.lock().ok().and_then(|network| network.clone()) {
        return Ok(network);
    }
    let channel = channel()?;
    let host = host_address()?;
    let (ssid, passphrase) = network();
    cx.router(RouterOperation::ApplyStationProfile {
        ssid: ssid.clone(),
        passphrase: passphrase.clone(),
        channel,
        forward_host: Some(host),
    })?;
    if let Ok(mut network) = NETWORK.lock() {
        *network = Some((ssid.clone(), passphrase.clone()));
    }
    Ok((ssid, passphrase))
}

/// The class, kind and detail of a failure code
/// (`oer_services_wifi_station::runtime`).
fn describe(code: u32) -> String {
    let [class, kind, high, low] = code.to_be_bytes();
    format!(
        "class {class} kind {kind} detail {}",
        u16::from_be_bytes([high, low])
    )
}

/// Whether failure `code` is the 4-way handshake's refusal of a wrong
/// passphrase on this stand's access point (`oer_services_wifi_station::
/// runtime` numbering): the station's `HandshakeFailed` (class 1, kind 6),
/// or the access point's deauthentication with reason 15, 4-way handshake
/// timeout (class 1, kind 1, detail 15; IEEE 802.11-2020 Table 9-49).
fn handshake_refused(code: u32) -> bool {
    matches!(code.to_be_bytes(), [1, 6, _, _] | [1, 1, 0, 15])
}

/// The router brings up a generated WPA2 network forwarding TCP and UDP to
/// this host; a connect with another passphrase ends with a handled
/// failure, the same boot answers on, and the station holds no link.
fn wrong_passphrase(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let (ssid, passphrase) = the_network(cx)?;
    let wrong = format!("not-{passphrase}");
    let answer = cx.call(connect(&ssid, &wrong)?, CONNECT_TIMEOUT)?;
    let Connection::Failed { code } = answer else {
        return Err(Failure::Fail(format!(
            "a wrong passphrase answered {answer:?}"
        )));
    };
    println!("wrong passphrase: {}", describe(code));
    check(
        handshake_refused(code),
        format!(
            "a wrong passphrase failed with code {code:#010x} ({}), not the 4-way handshake",
            describe(code)
        ),
    )?;
    let after = status(cx)?;
    check(!after.link_up, format!("after the failure: {after:?}"))
}

/// The device joins with the right passphrase on the router's channel; the
/// router's station table shows it authorized.
fn join(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let channel = channel()?;
    let (ssid, passphrase) = the_network(cx)?;
    let answer = cx.call(connect(&ssid, &passphrase)?, CONNECT_TIMEOUT)?;
    let Connection::Linked {
        channel: linked, ..
    } = answer
    else {
        return Err(Failure::Fail(format!("the connect answered {answer:?}")));
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

/// Wait on the device for its address; the router leased it that address
/// and is its gateway.
fn leased(cx: &mut Cx<'_>, wait_ms: u32) -> Result<Ipv4, Failure> {
    let answer = cx.call(
        WaitIpv4 {
            timeout_ms: wait_ms,
        },
        milliseconds(wait_ms) + MARGIN,
    )?;
    let Address::Leased(ipv4) = answer else {
        return Err(Failure::Fail(format!(
            "the address wait answered {answer:?}"
        )));
    };
    let device = Ipv4Addr::from(ipv4.address).to_string();
    let station = mac(status(cx)?.address);
    let lease = cx.router(RouterOperation::Lease { mac: station })?;
    check(
        lease.get("address").and_then(serde_json::Value::as_str) == Some(device.as_str()),
        format!("the device has {device}, the router's lease is {lease}"),
    )?;
    check(
        ipv4.gateway == Some(ROUTER_ADDRESS),
        format!("the gateway is {:?}, not the router", ipv4.gateway),
    )?;
    Ok(ipv4)
}

fn dhcp(cx: &mut Cx<'_>) -> Result<(), Failure> {
    leased(cx, DHCP_WAIT_MS).map(|_| ())
}

/// A TCP echo peer on this host for one connection: what it received and
/// from whom.
struct EchoPeer {
    port: u16,
    served: mpsc::Receiver<Result<(SocketAddr, Vec<u8>), String>>,
}

impl EchoPeer {
    /// Listen on `host` for one connection within `within`, echo up to
    /// `MAX_LINE` bytes until the peer stops sending or the line ends.
    fn start(host: Ipv4Addr, within: Duration) -> Result<Self, Failure> {
        let listener = TcpListener::bind((host, 0))
            .map_err(|error| Failure::Incomplete(format!("the echo peer's socket: {error}")))?;
        let port = listener
            .local_addr()
            .map_err(|error| Failure::Incomplete(format!("the echo peer's address: {error}")))?
            .port();
        listener
            .set_nonblocking(true)
            .map_err(|error| Failure::Incomplete(format!("the echo peer's socket: {error}")))?;
        let (sender, served) = mpsc::channel();
        std::thread::spawn(move || {
            let _sent = sender.send(Self::serve(&listener, within));
        });
        Ok(Self { port, served })
    }

    fn serve(listener: &TcpListener, within: Duration) -> Result<(SocketAddr, Vec<u8>), String> {
        let started = Instant::now();
        let (mut stream, peer) = loop {
            match listener.accept() {
                Ok(accepted) => break accepted,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if started.elapsed() >= within {
                        return Err(format!("no connection within {within:?}"));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(format!("accept: {error}")),
            }
        };
        stream
            .set_nonblocking(false)
            .and_then(|()| stream.set_read_timeout(Some(within)))
            .map_err(|error| format!("the connection: {error}"))?;
        let mut line = Vec::new();
        let mut buffer = [0; MAX_LINE];
        while !line.ends_with(b"\n") && line.len() < MAX_LINE {
            let count = stream
                .read(&mut buffer)
                .map_err(|error| format!("read: {error}"))?;
            let Some(read) = buffer.get(..count).filter(|read| !read.is_empty()) else {
                break;
            };
            stream
                .write_all(read)
                .map_err(|error| format!("write: {error}"))?;
            line.extend_from_slice(read);
        }
        Ok((peer, line))
    }

    fn served(&self, within: Duration) -> Result<(SocketAddr, Vec<u8>), Failure> {
        self.served
            .recv_timeout(within)
            .map_err(|_| Failure::Incomplete("the echo peer did not finish".into()))?
            .map_err(|error| Failure::Fail(format!("the echo peer: {error}")))
    }
}

/// The device sends a line over TCP to an echo peer on this host through
/// the router and reads it back; the peer received that line from the
/// device's address.
fn exchange(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let host = host_address()?;
    let device = status(cx)?
        .ipv4
        .ok_or_else(|| Failure::Fail("the device has no address".into()))?;
    let peer = EchoPeer::start(host, milliseconds(EXCHANGE_MS) + MARGIN)?;
    let line = format!("OER-SDK {}\n", tag());
    let request = Exchange {
        peer: host.octets(),
        port: peer.port,
        line: heapless::Vec::from_slice(line.as_bytes())
            .map_err(|_| Failure::Incomplete("the line is too long".into()))?,
        timeout_ms: EXCHANGE_MS,
    };
    let answer = cx.call(request, milliseconds(EXCHANGE_MS) + MARGIN)?;
    let (from, received) = peer.served(milliseconds(EXCHANGE_MS) + MARGIN)?;
    check(
        answer == Exchanged::Echoed,
        format!("the exchange answered {answer:?}"),
    )?;
    check(
        from.ip() == std::net::IpAddr::V4(Ipv4Addr::from(device.address)),
        format!(
            "the peer was reached from {from}, the device is {}",
            Ipv4Addr::from(device.address)
        ),
    )?;
    check(
        received == line.as_bytes(),
        format!("the peer received {:?}", String::from_utf8_lossy(&received)),
    )
}

fn tcp(cx: &mut Cx<'_>) -> Result<(), Failure> {
    exchange(cx)
}

/// The router deauthenticates the station: its link goes down, it rejoins
/// by itself, takes an address again and exchanges a line again.
fn reconnect(cx: &mut Cx<'_>) -> Result<(), Failure> {
    deauthenticated(cx)?;
    leased(cx, REJOIN_WAIT_MS)?;
    exchange(cx)
}

/// Deauthenticate the station on the router; the device sees its link go
/// down.
fn deauthenticated(cx: &mut Cx<'_>) -> Result<(), Failure> {
    let station = mac(status(cx)?.address);
    cx.router(RouterOperation::Deauthenticate { mac: station })?;
    let answer = cx.call(
        WaitLinkDown {
            timeout_ms: LINK_WAIT_MS,
        },
        milliseconds(LINK_WAIT_MS) + MARGIN,
    )?;
    check(
        answer == LinkLoss::Down,
        format!("the link-loss wait answered {answer:?}"),
    )
}

/// The router stops serving DHCP and deauthenticates the station: the
/// station rejoins, and its wait for an address times out without a reset.
/// The router serves again and the station takes an address.
fn no_dhcp(cx: &mut Cx<'_>) -> Result<(), Failure> {
    cx.router(RouterOperation::StationDhcp { serve: false })?;
    let without = (|| {
        deauthenticated(cx)?;
        cx.call(
            WaitIpv4 {
                timeout_ms: NO_DHCP_WAIT_MS,
            },
            milliseconds(NO_DHCP_WAIT_MS) + MARGIN,
        )
    })();
    // The router serves again whatever the device answered.
    cx.router(RouterOperation::StationDhcp { serve: true })?;
    let answer = without?;
    check(
        answer == Address::TimedOut,
        format!("without DHCP the address wait answered {answer:?}"),
    )?;
    let after = status(cx)?;
    check(after.link_up, format!("without DHCP: {after:?}"))?;
    leased(cx, REJOIN_WAIT_MS).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_network_and_its_wrong_passphrase_fit_the_service() {
        let (ssid, passphrase) = network();
        assert!(ssid.len() <= oer_services_sdk_station::MAX_SSID, "{ssid}");
        let wrong = format!("not-{passphrase}");
        for passphrase in [&passphrase, &wrong] {
            assert!(
                (8..=oer_services_sdk_station::MAX_PASSPHRASE).contains(&passphrase.len()),
                "{passphrase}"
            );
        }
        assert!(connect(&ssid, &wrong).is_ok());
    }

    #[test]
    fn an_exchange_line_fits_the_service() {
        let line = format!("OER-SDK {}\n", tag());
        assert!(line.len() <= MAX_LINE, "{line}");
    }

    #[test]
    fn only_the_handshakes_refusal_passes_for_a_wrong_passphrase() {
        // The station's handshake failure, and the AP's reason 15.
        assert!(handshake_refused(0x0106_0000));
        assert!(handshake_refused(0x0101_000f));
        // Not found (full and partial scan), authentication and association
        // failures, another peer reason, a refusal or a queue that did not
        // admit the connect.
        for code in [
            0x0105_0000,
            0x0105_0001,
            0x0102_0000,
            0x0104_0000,
            0x0101_0002,
            0x0200_0000,
            0x0300_0000,
        ] {
            assert!(!handshake_refused(code), "{code:#010x}");
        }
    }

    #[test]
    fn a_failure_code_names_its_class_kind_and_detail() {
        assert_eq!(describe(0x0106_000f), "class 1 kind 6 detail 15");
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
    }
}
