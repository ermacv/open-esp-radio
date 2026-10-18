//! Small session-identified UDP recovery proof for role-neutral radio cycles.

use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    time::{Duration, Instant},
};

use oer_hil_protocol::{
    network::Completion, network::Direction, network::FlowConfig, network::Ipv4Endpoint,
    network::SessionConfig, network::SessionFlowConfig, network::SessionLinkRequirements,
    network::Transport, network::UdpProbe, network::UdpSessionPayloadIdentity,
    wifi::WifiNetworkInterface,
};

use crate::Result;
use oer_hil_family_ieee80211_fixture::host_network::BenchmarkIpv4Route;
use oer_hil_link::SerialCapture;
use oer_hil_net_traffic::NetworkSession as _;
use oer_hil_net_traffic::{SessionEvidence, probe_udp_rx_ready, udp::confirm_reverse_flow};
use oer_hil_workload::context::Context;

const DEVICE_RX_PORT: u16 = 4_323;
const DEVICE_TX_SOURCE_PORT: u16 = 4_324;
const PAYLOAD_BYTES: usize = 64;
const RX_DATAGRAMS: u64 = 4;
const MAX_TX_DATAGRAMS: u64 = 16;
const SESSION_DURATION: Duration = Duration::from_secs(3);
const TARGET_TX_OFFER_BPS: u64 = 1_000;
const REVERSE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HostOffer {
    datagrams: u64,
    bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HostReceipt {
    datagrams: u64,
    bytes: u64,
}

pub(in crate::workload::ieee80211) fn prove_station_data_path(
    capture: &SerialCapture,
    context: &Context<'_>,
    timeout: Duration,
    stage: &str,
    network_cursor: usize,
) -> Result<()> {
    let address = capture.wait_for_network_ready_after(
        network_cursor,
        WifiNetworkInterface::Station,
        timeout,
    )?;
    let ready = probe_udp_rx_ready(capture, address, DEVICE_RX_PORT, timeout)?;
    if ready.address != address {
        return Err(format!(
            "station recovery changed its network endpoint from {address} to {}",
            ready.address,
        )
        .into());
    }

    let sink = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    sink.connect(SocketAddrV4::new(address, DEVICE_TX_SOURCE_PORT))?;
    let SocketAddr::V4(local) = sink.local_addr()? else {
        return Err("station recovery selected a non-IPv4 Host endpoint".into());
    };
    let route = BenchmarkIpv4Route::discover(address, &context.lab.station_fixture)?;
    route.verify_socket_source(*local.ip())?;
    confirm_reverse_flow(&sink, REVERSE_PROBE_TIMEOUT.min(timeout))?;

    let sender = UdpSocket::bind(SocketAddrV4::new(*local.ip(), 0))?;
    sender.connect(SocketAddrV4::new(address, DEVICE_RX_PORT))?;
    let (session, identity) = capture.start_identified_udp_session(|identity| SessionConfig {
        network_interface: WifiNetworkInterface::Station,
        transport: Transport::Udp,
        direction: Direction::Bidirectional,
        completion: Completion::DurationMillis(SESSION_DURATION.as_millis() as u32),
        flows: [
            Some(SessionFlowConfig {
                flow_id: 0,
                peer: Some(Ipv4Endpoint {
                    address: local.ip().octets(),
                    port: local.port(),
                }),
                target_rx: Some(FlowConfig {
                    payload_bytes: PAYLOAD_BYTES as u16,
                    offered_rate_bps: None,
                    pacing_group_datagrams: None,
                }),
                target_tx: Some(FlowConfig {
                    payload_bytes: PAYLOAD_BYTES as u16,
                    offered_rate_bps: Some(TARGET_TX_OFFER_BPS),
                    pacing_group_datagrams: Some(1),
                }),
                payload_identity: Some(identity),
            }),
            None,
        ],
        link_requirements: SessionLinkRequirements::NONE,
    })?;
    let offer = send_rx_payloads(&sender, identity)?;
    let structured = capture.wait_for_session(session, timeout)?;
    let receipt = receive_tx_payloads(
        &sink,
        identity,
        structured.transport.tx_units,
        DELIVERY_TIMEOUT,
    )?;
    require_recovery_exchange(offer, receipt, structured)?;
    capture.acknowledge_session(session)?;
    eprintln!(
        "wifi_radio_lifecycle_data_path={stage} address={address} identity={} target_rx={} host_tx={}",
        identity.value(),
        structured.transport.rx_units,
        receipt.datagrams,
    );
    Ok(())
}

fn send_rx_payloads(socket: &UdpSocket, identity: UdpSessionPayloadIdentity) -> Result<HostOffer> {
    let mut payload = [0x5a; PAYLOAD_BYTES];
    if !identity.write_to(&mut payload) {
        return Err("station recovery payload cannot carry its identity".into());
    }
    for sequence in 0..RX_DATAGRAMS {
        payload[..4].copy_from_slice(&(sequence as u32).to_be_bytes());
        require_full_send(socket.send(&payload)?)?;
    }
    payload[..4].copy_from_slice(&(-1_i32).to_be_bytes());
    require_full_send(socket.send(&payload)?)?;
    Ok(HostOffer {
        datagrams: RX_DATAGRAMS,
        bytes: RX_DATAGRAMS * PAYLOAD_BYTES as u64,
    })
}

fn require_full_send(length: usize) -> Result<()> {
    if length != PAYLOAD_BYTES {
        return Err(format!("short station recovery UDP send: {length}/{PAYLOAD_BYTES}").into());
    }
    Ok(())
}

fn receive_tx_payloads(
    socket: &UdpSocket,
    identity: UdpSessionPayloadIdentity,
    expected: u64,
    timeout: Duration,
) -> Result<HostReceipt> {
    if !(1..=MAX_TX_DATAGRAMS).contains(&expected) {
        return Err(format!(
            "bounded station recovery expected 1..={MAX_TX_DATAGRAMS} target TX datagrams, got {expected}"
        )
        .into());
    }
    socket.set_read_timeout(Some(Duration::from_millis(100).min(timeout)))?;
    let deadline = Instant::now() + timeout;
    let mut packet = [0; PAYLOAD_BYTES + 1];
    let mut received = 0_u64;
    while received < expected && Instant::now() < deadline {
        let (length, source) = match socket.recv_from(&mut packet) {
            Ok(packet) => packet,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if UdpProbe::decode(&packet[..length]).is_some() {
            continue;
        }
        if source != socket.peer_addr()?
            || length != PAYLOAD_BYTES
            || !identity.matches(&packet[..length])
        {
            return Err(format!(
                "station recovery Host received foreign or altered target TX payload: source={source} length={length} identity={} expected_peer={}",
                identity.value(), socket.peer_addr()?,
            ).into());
        }
        let sequence = u32::from_be_bytes(packet[..4].try_into()?);
        if sequence != received as u32 {
            return Err(format!(
                "station recovery target TX sequence {sequence} followed {} accepted datagrams",
                received,
            )
            .into());
        }
        received += 1;
    }
    if received != expected {
        return Err(format!(
            "station recovery delivered {received}/{expected} target TX datagrams before the bounded deadline"
        ).into());
    }
    Ok(HostReceipt {
        datagrams: received,
        bytes: received * PAYLOAD_BYTES as u64,
    })
}

fn require_recovery_exchange(
    offer: HostOffer,
    receipt: HostReceipt,
    target: SessionEvidence,
) -> Result<()> {
    let transport = target.transport;
    if !target.finished.summary.verdict.passed()
        || transport.transport_errors != 0
        || offer.datagrams != RX_DATAGRAMS
        || offer.bytes != RX_DATAGRAMS * PAYLOAD_BYTES as u64
        || transport.rx_delivered_units() != offer.datagrams
        || transport.rx_delivered_bytes() != offer.bytes
        || transport.tx_units != receipt.datagrams
        || transport.tx_bytes != receipt.bytes
        || receipt.datagrams == 0
    {
        return Err(format!(
            "station recovery RX+TX session did not reconcile: offered={offer:?} received={receipt:?} target={transport:?} verdict={}",
            target.finished.summary.verdict,
        ).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
