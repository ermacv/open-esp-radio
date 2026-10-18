//! Single-client UDP workload execution and delivery assessment.

use oer_hil_run_bundle_format::run::fixtures::Burst;
use std::{
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    time::Duration,
};

use oer_hil_protocol::{
    network::Completion, network::FlowConfig, network::Ipv4Endpoint, network::SessionConfig,
    network::SessionFlowConfig, network::SessionLinkRequirements, network::Transport,
};

use crate::scenario::{Direction, access_point::UdpCriteria};
use crate::workload::ieee80211::access_point::{
    Config, ConnectedClients, UDP_HOST_PORT, UDP_RX_PORT, UDP_TX_SOURCE_PORT, protocol_direction,
    report::TrafficReport, session_report, validate_rate_criteria,
};
use crate::{Result, workload::traffic::tx_traffic::Receiver};
use oer_hil_family_ieee80211_evidence as evidence;
use oer_hil_family_ieee80211_fixture::host_network::{BenchmarkIpv4Route, RouteMedium};
use oer_hil_link::SerialCapture;
use oer_hil_net_traffic::NetworkSession as _;
use oer_hil_net_traffic::{
    SessionEvidence, probe_udp_rx_ready_via, udp::configure_qualification_receive_buffer,
};
use oer_hil_net_traffic::{
    paced_udp::Config as UdpConfig, paced_udp::HostTransmission as UdpTransmission,
    paced_udp::send as send_udp,
};

#[derive(Clone, Copy)]
pub(super) struct UdpWorkload {
    pub(super) direction: Direction,
    pub(super) duration: Duration,
    pub(super) rx_rate_bps: Option<u64>,
    pub(super) tx_rate_bps: Option<u64>,
    pub(super) secondary_rx_rate_bps: Option<u64>,
    pub(super) secondary_tx_rate_bps: Option<u64>,
    pub(super) secondary_tx_pacing_group_datagrams: Option<u8>,
    pub(super) payload_bytes: usize,
}

#[derive(Clone, Copy)]
pub(super) struct UdpEvidencePolicy {
    pub(super) exact_delivery: bool,
    pub(super) driver_observation: bool,
    pub(super) rx_delivery: bool,
}

pub(super) fn qualify_udp(
    output: &std::path::Path,
    capture: &SerialCapture,
    config: &Config,
    context: &oer_hil_workload::context::Context<'_>,
    clients: &ConnectedClients,
    workload: UdpWorkload,
    criteria: UdpCriteria,
) -> Result<TrafficReport> {
    let UdpWorkload {
        direction,
        duration,
        rx_rate_bps,
        tx_rate_bps,
        payload_bytes,
        ..
    } = workload;
    let protocol_direction = protocol_direction(direction);
    let target = context.lab.access_point.target_address();
    let traffic_target = clients.traffic_target(target)?;
    let host = context.lab.access_point.client_address();
    // The host's route to the target: over the air from the laptop client,
    // over Ethernet to an OpenWrt client. Every datagram leaves from the
    // route's source address, and the route is recorded with the run.
    let route = BenchmarkIpv4Route::discover_over(
        traffic_target,
        Some(if clients.openwrt_primary().is_some() {
            RouteMedium::Ethernet
        } else {
            RouteMedium::Wireless
        }),
    )?;
    if rx_rate_bps.is_some() {
        probe_udp_rx_ready_via(
            capture,
            oer_hil_protocol::wifi::WifiNetworkInterface::AccessPoint,
            target,
            (traffic_target != target).then_some(traffic_target),
            UDP_RX_PORT,
            config.timeout,
        )?;
    }
    let socket = if tx_rate_bps.is_some() {
        let bind_address = if clients.openwrt_primary().is_some() {
            Ipv4Addr::UNSPECIFIED
        } else {
            host
        };
        let socket = UdpSocket::bind(SocketAddrV4::new(bind_address, UDP_HOST_PORT))?;
        configure_qualification_receive_buffer(&socket)?;
        socket.connect(SocketAddrV4::new(traffic_target, UDP_TX_SOURCE_PORT))?;
        oer_hil_net_traffic::prepare_udp_reverse_flow(
            capture,
            oer_hil_protocol::wifi::WifiNetworkInterface::AccessPoint,
            &socket,
            config.timeout,
        )?;
        Some(socket)
    } else {
        None
    };
    let duration_millis = u32::try_from(duration.as_millis())?;
    let receiver = socket
        .as_ref()
        .map(|socket| {
            Receiver::start(
                socket,
                traffic_target,
                config.timeout + duration + oer_hil_net_traffic::SESSION_START_TIMEOUT,
                output,
                "primary",
            )
        })
        .transpose()?;
    let session = capture.start_session(SessionConfig {
        network_interface: oer_hil_protocol::wifi::WifiNetworkInterface::AccessPoint,
        transport: Transport::Udp,
        direction: protocol_direction,
        completion: Completion::DurationMillis(duration_millis),
        flows: [
            Some(SessionFlowConfig {
                flow_id: 0,
                peer: tx_rate_bps.map(|_| Ipv4Endpoint {
                    address: host.octets(),
                    port: UDP_HOST_PORT,
                }),
                target_rx: rx_rate_bps.map(|rate| FlowConfig {
                    payload_bytes: u16::try_from(payload_bytes).expect("validated UDP payload"),
                    offered_rate_bps: Some(rate),
                    pacing_group_datagrams: None,
                }),
                target_tx: tx_rate_bps.map(|rate| FlowConfig {
                    payload_bytes: u16::try_from(payload_bytes).expect("validated UDP payload"),
                    offered_rate_bps: Some(rate),
                    pacing_group_datagrams: None,
                }),
                payload_identity: None,
            }),
            None,
        ],
        // Link negotiation is production state and is verified from the
        // measured interval. It is not a load-generator pacing control.
        link_requirements: SessionLinkRequirements::NONE,
    })?;
    let send_config = rx_rate_bps.map(|rate| UdpConfig {
        address: traffic_target,
        port: UDP_RX_PORT,
        rate_bps: rate,
        duration,
        payload: payload_bytes,
    });
    let host_tx = send_config
        .map(|config| send_udp(route.source(), config))
        .transpose();
    let structured = capture.wait_for_session(session, config.timeout);
    if let Ok(evidence) = &structured
        && let Some(load) = &evidence.cpu_load
    {
        context.measurements.cpu_load(load);
    }
    let acknowledgement = structured
        .as_ref()
        .map(|_| capture.acknowledge_session(session))
        .unwrap_or(Ok(()));
    let host_rx = receiver
        .map(|receiver| {
            receiver.finish(
                structured
                    .as_ref()
                    .ok()
                    .map(|evidence| evidence.transport.tx_units),
            )
        })
        .transpose();
    let host_tx = host_tx
        .map_err(|error| oer_hil_link::error::context("AP UDP host sender failed", error))?;
    if let Some(host) = host_tx {
        route.record(output, traffic_target, host.source)?;
    }
    let host_rx = host_rx
        .map_err(|error| oer_hil_link::error::context("AP UDP host receiver failed", error))?;
    let structured = structured.map_err(|error| format!("AP UDP target failed: {error}"))?;
    acknowledgement?;
    let mut report = session_report(direction, &structured);
    let host_received = validate_udp(
        host_tx,
        host_rx.as_deref(),
        structured,
        UdpEvidencePolicy {
            exact_delivery: criteria.exact_delivery,
            driver_observation: config.require_driver_observation,
            rx_delivery: config.require_rx_delivery_evidence,
        },
    )?;
    if let Some(host_received) = host_received {
        // Target TX evidence counts frames admitted to the radio path. Under
        // an intentionally saturated characterization workload, only the
        // receiver can report delivered throughput.
        report.tx_bytes = host_received.bytes;
        report.tx_units = host_received.datagrams;
    }
    validate_rate_criteria(&report, criteria.floors(), &context.measurements).map_err(|error| {
        let source = host_tx.map(|host| {
            format!(
                "host UDP source={}bps datagrams={} maximum_lateness_us={} maximum_catch_up_datagrams={} deadline_resets={}",
                host.throughput_bps(),
                host.datagrams,
                host.maximum_lateness_us(),
                host.maximum_catch_up_datagrams,
                host.deadline_resets,
            )
        });
        match source {
            Some(source) => format!("{error}; {source}"),
            None => error.to_string(),
        }
    })?;
    Ok(TrafficReport::Udp(report))
}

pub(super) fn validate_udp(
    host_tx: Option<UdpTransmission>,
    host_rx: Option<&[Burst]>,
    evidence: SessionEvidence,
    policy: UdpEvidencePolicy,
) -> Result<Option<Burst>> {
    if !evidence.finished.summary.verdict.passed() || evidence.transport.transport_errors != 0 {
        return Err(format!(
            "AP UDP target failed: verdict={} errors={}",
            evidence.finished.summary.verdict, evidence.transport.transport_errors,
        )
        .into());
    }
    match host_tx {
        Some(host) => {
            // Delivery is every datagram before the host's terminal marker,
            // inside the throughput window or after it.
            let (delivered_bytes, delivered_units) = (
                evidence.transport.rx_delivered_bytes(),
                evidence.transport.rx_delivered_units(),
            );
            if (policy.exact_delivery
                && (host.bytes != delivered_bytes || host.datagrams != delivered_units))
                || (!policy.exact_delivery
                    && (delivered_bytes > host.bytes || delivered_units > host.datagrams))
            {
                return Err(format!(
                    "AP UDP RX mismatch: host={}/{} target={}/{} (late {}/{})",
                    host.bytes,
                    host.datagrams,
                    delivered_bytes,
                    delivered_units,
                    evidence.transport.rx_late_bytes,
                    evidence.transport.rx_late_units,
                )
                .into());
            }
            if policy.driver_observation {
                let rx = evidence
                    .radio
                    .and_then(|radio| radio.rx)
                    .ok_or("AP UDP RX session did not publish typed RX radio evidence")?;
                let expected_highest = policy
                    .exact_delivery
                    .then(|| {
                        u32::try_from(host.datagrams)
                            .ok()
                            .and_then(|datagrams| datagrams.checked_sub(1))
                    })
                    .flatten();
                if (policy.exact_delivery
                    && (rx.sequence_first != Some(0)
                        || rx.sequence_highest != expected_highest
                        || rx.sequence_gap_events != 0
                        || rx.sequence_forward_missing != 0))
                    || rx.sequence_backward != 0
                    || rx.sequence_duplicates != 0
                    || rx.sequence_unsequenced != 0
                {
                    return Err(format!("AP UDP RX ordering defect: {rx:?}").into());
                }
            }
            if policy.rx_delivery {
                let delivery = evidence
                    .rx_delivery
                    .ok_or("AP diagnostic did not publish typed RX delivery evidence")?;
                let assessment = evidence::rx_delivery::assess(host.datagrams, delivery);
                if !assessment.exact() {
                    return Err(format!(
                        "typed AP RX delivery frontier is {}",
                        assessment.frontier()
                    )
                    .into());
                }
            }
        }
        None if evidence.transport.rx_bytes != 0 || evidence.transport.rx_units != 0 => {
            return Err("AP UDP TX-only session reported received traffic".into());
        }
        None => {}
    }
    match host_rx {
        Some(bursts) => {
            let qualified: Vec<_> = bursts
                .iter()
                .copied()
                .filter(|burst| burst.started_at_zero)
                .collect();
            if qualified.len() != 1 {
                return Err(format!(
                    "AP UDP TX produced {} zero-started bursts instead of one",
                    qualified.len()
                )
                .into());
            }
            let host = qualified[0];
            if (policy.exact_delivery && host.missing != 0)
                || host.reordered != 0
                || host.duplicates != 0
            {
                return Err(format!(
                    "AP UDP TX ordering defect: missing={} reordered={} duplicates={} \
                     sequence_range={}..={} missing_runs={} largest_missing_run={} \
                     largest_missing_range={:?}..={:?} maximum_interarrival_us={} \
                     sequence_after_maximum_interarrival={:?}",
                    host.missing,
                    host.reordered,
                    host.duplicates,
                    host.lowest_sequence,
                    host.highest_sequence,
                    host.missing_runs,
                    host.maximum_missing_run,
                    host.maximum_missing_run_start,
                    host.maximum_missing_run_end,
                    host.maximum_interarrival_us,
                    host.sequence_after_maximum_interarrival,
                )
                .into());
            }
            if (policy.exact_delivery
                && (host.bytes != evidence.transport.tx_bytes
                    || host.datagrams != evidence.transport.tx_units))
                || (!policy.exact_delivery
                    && (host.bytes > evidence.transport.tx_bytes
                        || host.datagrams > evidence.transport.tx_units))
            {
                return Err(format!(
                    "AP UDP TX mismatch: target={}/{} host={}/{}",
                    evidence.transport.tx_bytes,
                    evidence.transport.tx_units,
                    host.bytes,
                    host.datagrams,
                )
                .into());
            }
            return Ok(Some(host));
        }
        None if evidence.transport.tx_bytes != 0 || evidence.transport.tx_units != 0 => {
            return Err("AP UDP RX-only session reported transmitted traffic".into());
        }
        None => {}
    }
    Ok(None)
}
