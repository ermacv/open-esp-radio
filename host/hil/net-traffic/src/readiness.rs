//! Readiness of the target's network services: the typed declarations a
//! session waits for, and the end-to-end UDP probes that prove a receive
//! service before a measured session starts.

use std::{
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    time::{Duration, Instant},
};

use oer_hil_link::{Result, SerialCapture};
use oer_hil_protocol::network::{Direction, Transport, WifiNetworkInterface};

const RX_PROBE_PAYLOAD: usize = 64;
const RX_PROBE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

/// The target's address once its UDP receive service is proven.
#[derive(Clone, Copy, Debug)]
pub struct UdpRxReady {
    pub address: Ipv4Addr,
}

/// Whether the current boot declared the service `transport`/`direction`
/// on `port` of `network_interface`.
pub(crate) fn observed_service(
    capture: &SerialCapture,
    network_interface: WifiNetworkInterface,
    transport: Transport,
    direction: Direction,
    port: u16,
) -> bool {
    let Some(boot_id) = capture.latest_boot_id() else {
        return false;
    };
    capture
        .messages_since(0, |messages| {
            messages
                .iter()
                .filter(|message| message.boot_id == boot_id)
                .any(|message| match message.decode() {
                    Some(oer_hil_protocol::network::ServiceReady(service)) => {
                        service.network_interface == network_interface
                            && service.transport == transport
                            && service.direction == direction
                            && service.local_port == port
                    }
                    None => false,
                })
        })
        .unwrap_or(false)
}

/// Wait for both typed declarations from the current boot. The cursor is
/// captured before inspecting state, so an event between inspection and sleep
/// cannot be lost. A timeout is failure, never permission to use an IP hint.
pub(crate) fn wait_for_services(
    capture: &SerialCapture,
    interface: WifiNetworkInterface,
    services: &[(Transport, Direction, u16)],
    timeout: Duration,
) -> Result<Ipv4Addr> {
    let deadline = Instant::now() + timeout;
    loop {
        let cursor = capture.event_cursor();
        capture.check_link()?;
        if let Some(address) = capture.observed_protocol_ipv4(interface)
            && services.iter().all(|&(transport, direction, port)| {
                observed_service(capture, interface, transport, direction, port)
            })
        {
            return Ok(address);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "device did not publish {interface:?} network and service readiness: {services:?}"
            )
            .into());
        }
        capture.wait_for_message_after(cursor, remaining, |_| true)?;
    }
}

/// Prime only an event-confirmed bound UDP socket, before measured Start.
pub fn prepare_udp_reverse_flow(
    capture: &SerialCapture,
    interface: WifiNetworkInterface,
    socket: &UdpSocket,
    timeout: Duration,
) -> Result<()> {
    wait_for_services(
        capture,
        interface,
        &[(Transport::Udp, Direction::Tx, socket.peer_addr()?.port())],
        timeout,
    )?;
    crate::udp::confirm_reverse_flow(socket, timeout)?;
    Ok(())
}

/// Prove the already-running UDP RX service from its current network peer.
///
/// It does not prepare or assume the station role. Call it only after the
/// controlled client has joined, so the unmeasured datagram also establishes
/// the AP-side neighbor/data path before sequence zero is admitted to a
/// measured session.
pub fn probe_udp_rx_ready(
    capture: &SerialCapture,
    address_hint: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> Result<UdpRxReady> {
    probe_udp_rx_ready_via(
        capture,
        WifiNetworkInterface::Station,
        address_hint,
        None,
        port,
        timeout,
    )
}

/// Prove UDP RX readiness while an explicit routed peer owns the host path.
///
/// `address_hint` remains the target identity published in typed evidence;
/// `traffic_address` is only the address through which the probe reaches it.
/// This distinction is required when a controlled OpenWrt Wi-Fi station
/// forwards traffic from the wired HIL generator to the DUT AP.
pub fn probe_udp_rx_ready_via(
    capture: &SerialCapture,
    network_interface: WifiNetworkInterface,
    address_hint: Ipv4Addr,
    traffic_address: Option<Ipv4Addr>,
    port: u16,
    timeout: Duration,
) -> Result<UdpRxReady> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    let mut address = address_hint;
    let mut connected_address = traffic_address.unwrap_or(address);
    if !connected_address.is_unspecified() {
        socket.connect(SocketAddrV4::new(connected_address, port))?;
    }
    socket.set_write_timeout(Some(Duration::from_millis(250)))?;
    let mut packet = [0x5a; RX_PROBE_PAYLOAD];
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        let event_start = capture.event_cursor();
        capture.check_link()?;
        if let Some(discovered) = capture.observed_protocol_ipv4(network_interface)
            && discovered != address
        {
            address = discovered;
            if traffic_address.is_none() {
                connected_address = address;
                socket.connect(SocketAddrV4::new(connected_address, port))?;
            }
        }
        let rx_service_ready = observed_service(
            capture,
            network_interface,
            Transport::Udp,
            Direction::Rx,
            port,
        );
        let tx_service_ready = observed_service(
            capture,
            network_interface,
            Transport::Udp,
            Direction::Tx,
            4_324,
        );
        if !rx_service_ready
            || !tx_service_ready
            || capture.observed_protocol_ipv4(network_interface).is_none()
        {
            capture.wait_for_message_after(
                event_start,
                deadline.saturating_duration_since(Instant::now()),
                |_| true,
            )?;
            continue;
        }

        let boot_id = capture
            .latest_boot_id()
            .ok_or("device did not publish a current HIL boot identity")?;
        let event_start = capture.event_cursor();
        packet[..4].copy_from_slice(&(-1_i32).to_be_bytes());
        socket.send(&packet)?;
        if capture
            .wait_for_after(
                event_start,
                RX_PROBE_RESPONSE_TIMEOUT.min(deadline.saturating_duration_since(Instant::now())),
                |message, oer_hil_protocol::network::ServiceReady(service)| {
                    message.boot_id == boot_id
                        && service.network_interface == network_interface
                        && service.transport == Transport::Udp
                        && service.direction == Direction::Rx
                        && service.local_port == port
                },
            )?
            .is_some()
        {
            return Ok(UdpRxReady { address });
        }
    }

    Err(format!(
        "device {address}:{port} did not confirm end-to-end UDP RX within {} seconds",
        timeout.as_secs(),
    )
    .into())
}
