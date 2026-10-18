//! Concurrent-client UDP execution and per-flow fairness assessment.

use oer_hil_net_traffic::NetworkSession as _;
use oer_hil_run_bundle_format::run::fixtures::Burst;
use oer_hil_workload::context::Context;
use std::{
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::Path,
    thread,
    time::Duration,
};

use oer_hil_protocol::{
    network::Completion, network::FlowConfig, network::FlowTransportEvidence,
    network::Ipv4Endpoint, network::SESSION_FLOW_CAPACITY, network::SessionConfig,
    network::SessionFlowConfig, network::SessionLinkRequirements, network::Transport,
};

use crate::scenario::{Direction, access_point::MultiClientCriteria};
use crate::workload::ieee80211::access_point::{
    Config, ConnectedClients, UDP_HOST_PORT, UDP_RX_PORT, UDP_SECONDARY_HOST_PORT,
    UDP_TX_SOURCE_PORT, protocol_direction,
    report::{MultiClientFlowReport, MultiClientSessionReport, SessionReport, TrafficReport},
    udp::UdpWorkload,
    validate_rate_criteria,
};
use crate::{Result, workload::traffic::tx_traffic::Receiver};
use oer_hil_link::SerialCapture;
use oer_hil_net_traffic::{
    paced_udp::Config as UdpConfig, paced_udp::HostTransmission as UdpTransmission,
    paced_udp::send_on as send_udp_on,
};
use oer_hil_net_traffic::{probe_udp_rx_ready_via, udp::configure_qualification_receive_buffer};

struct MultiClientHostFlow {
    flow_id: u8,
    peer: Ipv4Endpoint,
    traffic_target: Ipv4Addr,
    socket: UdpSocket,
}

const fn target_receives(direction: Direction) -> bool {
    matches!(direction, Direction::Rx | Direction::Bidirectional)
}

const fn target_transmits(direction: Direction) -> bool {
    matches!(direction, Direction::Tx | Direction::Bidirectional)
}

fn multi_client_host_flows(
    context: &Context<'_>,
    clients: &ConnectedClients,
    direction: Direction,
) -> Result<[MultiClientHostFlow; SESSION_FLOW_CAPACITY]> {
    let ConnectedClients::Laptop {
        secondary: Some(secondary),
        ..
    } = clients
    else {
        return Err("multi-client AP UDP requires the laptop and OpenWrt clients".into());
    };
    let target = context.lab.access_point.target_address();
    let primary_peer = context.lab.access_point.client_address();
    let secondary_peer = context
        .lab
        .access_point
        .secondary_client_address()
        .ok_or("multi-client AP UDP requires access_point.secondary_client_address")?;
    let secondary_target = secondary
        .forward_address()
        .ok_or("secondary OpenWrt client omitted its wired forwarding address")?;

    let create = |flow_id: u8,
                  bind_address: Ipv4Addr,
                  port: u16,
                  peer: Ipv4Addr,
                  traffic_target: Ipv4Addr|
     -> Result<_> {
        let socket = open_multi_client_socket(
            SocketAddrV4::new(bind_address, port),
            target_transmits(direction).then_some(SocketAddrV4::new(
                traffic_target,
                UDP_TX_SOURCE_PORT + u16::from(flow_id),
            )),
        )?;
        Ok(MultiClientHostFlow {
            flow_id,
            peer: Ipv4Endpoint {
                address: peer.octets(),
                port,
            },
            traffic_target,
            socket,
        })
    };

    Ok([
        create(0, primary_peer, UDP_HOST_PORT, primary_peer, target)?,
        create(
            1,
            Ipv4Addr::UNSPECIFIED,
            UDP_SECONDARY_HOST_PORT,
            secondary_peer,
            secondary_target,
        )?,
    ])
}

fn open_multi_client_socket(
    bind: SocketAddrV4,
    reverse_peer: Option<SocketAddrV4>,
) -> Result<UdpSocket> {
    let socket = UdpSocket::bind(bind)?;
    configure_qualification_receive_buffer(&socket)?;
    if let Some(peer) = reverse_peer {
        socket.connect(peer)?;
    }
    Ok(socket)
}

/// The observation that keeps a cycle's raw delivery, named by the cycle's
/// directory.
fn delivery_progress(output: &Path) -> String {
    match output.file_name().and_then(|name| name.to_str()) {
        Some(cycle) => format!("{cycle}-delivery-progress"),
        None => String::from("delivery-progress"),
    }
}

pub(super) fn qualify_multi_client_udp(
    output: &Path,
    capture: &SerialCapture,
    config: &Config,
    context: &Context<'_>,
    clients: &ConnectedClients,
    workload: UdpWorkload,
    criteria: MultiClientCriteria,
) -> Result<TrafficReport> {
    if config.require_rx_delivery_evidence {
        return Err("multi-client AP UDP does not combine independent RX sequence ledgers".into());
    }
    let UdpWorkload {
        direction,
        duration,
        rx_rate_bps,
        tx_rate_bps,
        secondary_rx_rate_bps,
        secondary_tx_rate_bps,
        secondary_tx_pacing_group_datagrams,
        payload_bytes,
    } = workload;
    let flows = multi_client_host_flows(context, clients, direction)?;
    let target = context.lab.access_point.target_address();
    if target_receives(direction) {
        for flow in &flows {
            probe_udp_rx_ready_via(
                capture,
                oer_hil_protocol::wifi::WifiNetworkInterface::AccessPoint,
                target,
                (flow.traffic_target != target).then_some(flow.traffic_target),
                UDP_RX_PORT,
                config.timeout,
            )?;
        }
    }

    if target_transmits(direction) {
        for flow in &flows {
            oer_hil_net_traffic::prepare_udp_reverse_flow(
                capture,
                oer_hil_protocol::wifi::WifiNetworkInterface::AccessPoint,
                &flow.socket,
                config.timeout,
            )?;
        }
    }

    let payload_bytes_u16 =
        u16::try_from(payload_bytes).expect("validated multi-client UDP payload");
    let mut receivers = Vec::with_capacity(SESSION_FLOW_CAPACITY);
    if target_transmits(direction) {
        for flow in &flows {
            receivers.push((
                flow.flow_id,
                Receiver::start(
                    &flow.socket,
                    flow.traffic_target,
                    config.timeout + duration + oer_hil_net_traffic::SESSION_START_TIMEOUT,
                    output,
                    &format!("flow-{}", flow.flow_id),
                )?,
            ));
        }
    }
    let mut probe_source = if config.probe_load {
        Some(
            oer_hil_family_ieee80211_fixture::probe_load::Source::prepare(
                &oer_hil_family_ieee80211_fixture::probe_load::model::Config {
                    ssid: context.lab.access_point.credentials().0.to_owned(),
                    channel: context.lab.access_point.channel(),
                },
                output,
            )?,
        )
    } else {
        None
    };
    let session = capture.start_session(SessionConfig {
        network_interface: oer_hil_protocol::wifi::WifiNetworkInterface::AccessPoint,
        transport: Transport::Udp,
        direction: protocol_direction(direction),
        completion: Completion::DurationMillis(u32::try_from(duration.as_millis())?),
        flows: std::array::from_fn(|index| {
            let flow = &flows[index];
            let flow_rx_rate = if index == 0 {
                rx_rate_bps
            } else {
                secondary_rx_rate_bps.or(rx_rate_bps)
            };
            let flow_tx_rate = if index == 0 {
                tx_rate_bps
            } else {
                secondary_tx_rate_bps.or(tx_rate_bps)
            };
            Some(SessionFlowConfig {
                flow_id: flow.flow_id,
                peer: Some(flow.peer),
                target_rx: flow_rx_rate.map(|rate| FlowConfig {
                    payload_bytes: payload_bytes_u16,
                    offered_rate_bps: Some(rate),
                    pacing_group_datagrams: None,
                }),
                target_tx: flow_tx_rate.map(|rate| FlowConfig {
                    payload_bytes: payload_bytes_u16,
                    offered_rate_bps: Some(rate),
                    pacing_group_datagrams: (index == 1)
                        .then_some(secondary_tx_pacing_group_datagrams)
                        .flatten(),
                }),
                payload_identity: None,
            })
        }),
        link_requirements: SessionLinkRequirements::NONE,
    })?;

    if let Some(source) = probe_source.as_mut() {
        source.start()?;
    }
    let mut sender_threads = Vec::with_capacity(SESSION_FLOW_CAPACITY);
    if target_receives(direction) {
        for (index, flow) in flows.iter().enumerate() {
            let socket = flow.socket.try_clone()?;
            let flow_id = flow.flow_id;
            let rate_bps = if index == 0 {
                rx_rate_bps
            } else {
                secondary_rx_rate_bps.or(rx_rate_bps)
            }
            .expect("validated multi-client RX offer");
            let send_config = UdpConfig {
                address: flow.traffic_target,
                port: UDP_RX_PORT,
                rate_bps,
                duration,
                payload: payload_bytes,
            };
            sender_threads.push((
                flow_id,
                thread::spawn(move || send_udp_on(&socket, send_config)),
            ));
        }
    }

    let mut host_errors = Vec::new();
    let mut host_tx = [None; SESSION_FLOW_CAPACITY];
    for (flow_id, sender) in sender_threads {
        match sender.join() {
            Ok(Ok(sent)) => host_tx[usize::from(flow_id)] = Some(sent),
            Ok(Err(error)) => {
                if let Some(failure) =
                    error.downcast_ref::<oer_hil_net_traffic::paced_udp::SendFailure>()
                {
                    host_tx[usize::from(flow_id)] = Some(failure.progress);
                }
                host_errors.push(format!("AP UDP flow {flow_id} sender failed: {error}"));
            }
            Err(_) => host_errors.push(format!("AP UDP flow {flow_id} sender thread panicked")),
        }
    }

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
    let mut host_rx: [Option<Vec<Burst>>; SESSION_FLOW_CAPACITY] = [None, None];
    for (flow_id, receiver) in &receivers {
        receiver.target_finished(
            structured
                .as_ref()
                .ok()
                .and_then(|evidence| evidence.flow_transport[usize::from(*flow_id)])
                .map(|flow| flow.tx_units),
        );
    }
    for (flow_id, receiver) in receivers {
        let expected = structured
            .as_ref()
            .ok()
            .and_then(|evidence| evidence.flow_transport[usize::from(flow_id)])
            .map(|flow| flow.tx_units);
        match receiver.finish(expected) {
            Ok(received) => host_rx[usize::from(flow_id)] = Some(received),
            Err(error) => {
                host_errors.push(format!("AP UDP flow {flow_id} receiver failed: {error}"))
            }
        }
    }

    if let Some(source) = probe_source
        && let Err(error) = source.finish()
    {
        host_errors.push(format!("probe source: {error}"));
    }
    let observed = MultiClientObservation {
        direction,
        duration,
        peers: flows.each_ref().map(|flow| flow.peer),
        host_requested_bps: [rx_rate_bps, secondary_rx_rate_bps.or(rx_rate_bps)],
        host_tx,
        host_rx,
        host_errors,
        target: structured.map(|evidence| MultiClientTarget {
            elapsed_micros: evidence.transport.elapsed_micros,
            flows: evidence.flow_transport,
        }),
    };
    let report = observed.evaluate(output, &criteria, &context.measurements, &context.results);
    acknowledgement?;
    report
}

struct MultiClientTarget {
    elapsed_micros: u64,
    flows: [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY],
}

struct MultiClientObservation {
    direction: Direction,
    duration: Duration,
    peers: [Ipv4Endpoint; SESSION_FLOW_CAPACITY],
    host_tx: [Option<UdpTransmission>; SESSION_FLOW_CAPACITY],
    host_requested_bps: [Option<u64>; SESSION_FLOW_CAPACITY],
    host_rx: [Option<Vec<Burst>>; SESSION_FLOW_CAPACITY],
    target: Result<MultiClientTarget>,
    host_errors: Vec<String>,
}

impl MultiClientObservation {
    fn evaluate(
        self,
        output: &Path,
        criteria: &MultiClientCriteria,
        measurements: &oer_hil_workload::measurements::Recorder,
        results: &oer_hil_workload::results::Results,
    ) -> Result<TrafficReport> {
        // Preserve delivery even when terminal evidence or a later gate fails.
        // Raw per-flow host observations must not depend on qualification.
        let host_offer: [_; SESSION_FLOW_CAPACITY] = std::array::from_fn(|index| {
            self.host_requested_bps[index].map(|rate| {
                oer_hil_net_traffic::offered_load::Assessment::new(
                    rate,
                    self.duration,
                    self.host_tx[index],
                    criteria.minimum_host_offer_percent,
                )
            })
        });
        let progress = serde_json::json!({
            "host_offer": host_offer,
            "direction": self.direction,
            "duration_micros": u64::try_from(self.duration.as_micros())?,
            "peers": self.peers,
            "host_tx": self.host_tx.map(|host| host.map(|host| serde_json::json!({
                "bytes": host.bytes,
                "datagrams": host.datagrams,
                "elapsed_micros": host.elapsed.as_micros(),
                "maximum_lateness_micros": host.maximum_lateness.as_micros(),
                "maximum_catch_up_datagrams": host.maximum_catch_up_datagrams,
                "deadline_resets": host.deadline_resets,
            }))),
            "host_rx": self.host_rx,
            "host_errors": self.host_errors,
            "target_flows": self.target.as_ref().ok().map(|target| target.flows),
            "target_elapsed_micros": self.target.as_ref().ok().map(|target| target.elapsed_micros),
            "target_error": self.target.as_ref().err().map(|error| error.to_string()),
        });
        results.replace(delivery_progress(output), &progress);
        if !self.host_errors.is_empty() {
            return Err(self.host_errors.join("; ").into());
        }
        for (index, offer) in host_offer.iter().enumerate() {
            if let Some(offer) = offer {
                offer.validate(index)?;
            }
        }
        let structured = self
            .target
            .map_err(|error| format!("AP UDP target failed: {error}"))?;
        let direction = self.direction;
        let mut flow_reports = [MultiClientFlowReport {
            flow_id: 0,
            peer: Ipv4Endpoint {
                address: [0; 4],
                port: 0,
            },
            rx_bytes: 0,
            tx_bytes: 0,
            rx_units: 0,
            tx_units: 0,
            elapsed_micros: 0,
            rx_bps: 0,
            tx_bps: 0,
            host_tx_started_at_zero: None,
            host_tx_missing: None,
            host_tx_reordered: None,
            host_tx_duplicates: None,
            host_tx_maximum_interarrival_us: None,
            host_tx_sequence_after_maximum_interarrival: None,
        }; SESSION_FLOW_CAPACITY];
        for (index, report) in flow_reports.iter_mut().enumerate() {
            let flow_evidence = structured.flows[index]
                .ok_or_else(|| format!("AP UDP flow {index} omitted transport evidence"))?;
            *report = validate_multi_client_udp_flow(
                self.peers[index],
                self.host_tx[index],
                self.host_rx[index].as_deref(),
                flow_evidence,
                criteria.exact_delivery,
            )?;
        }
        let aggregate = SessionReport {
            direction,
            rx_bytes: flow_reports.iter().map(|flow| flow.rx_bytes).sum(),
            tx_bytes: flow_reports.iter().map(|flow| flow.tx_bytes).sum(),
            rx_units: flow_reports.iter().map(|flow| flow.rx_units).sum(),
            tx_units: flow_reports.iter().map(|flow| flow.tx_units).sum(),
            elapsed_micros: structured.elapsed_micros,
        };
        validate_rate_criteria(&aggregate, criteria.floors(), measurements)?;
        validate_multi_client_fairness(&flow_reports, direction, criteria, measurements)?;
        Ok(TrafficReport::UdpMultiClient(Box::new(
            MultiClientSessionReport {
                direction,
                aggregate,
                flows: flow_reports,
            },
        )))
    }
}

fn validate_multi_client_udp_flow(
    peer: Ipv4Endpoint,
    host_tx: Option<UdpTransmission>,
    host_rx: Option<&[Burst]>,
    evidence: FlowTransportEvidence,
    exact_delivery: bool,
) -> Result<MultiClientFlowReport> {
    if evidence.transport_errors != 0 {
        return Err(format!(
            "AP UDP flow {} reported {} transport errors",
            evidence.flow_id, evidence.transport_errors,
        )
        .into());
    }
    let (rx_bytes, rx_units) = match host_tx {
        Some(host)
            if (exact_delivery
                && (host.bytes != evidence.rx_delivered_bytes()
                    || host.datagrams != evidence.rx_delivered_units()))
                || (!exact_delivery
                    && (evidence.rx_delivered_bytes() > host.bytes
                        || evidence.rx_delivered_units() > host.datagrams)) =>
        {
            return Err(format!(
                "AP UDP flow {} RX mismatch: host={}/{} target={}/{}",
                evidence.flow_id,
                host.bytes,
                host.datagrams,
                evidence.rx_delivered_bytes(),
                evidence.rx_delivered_units(),
            )
            .into());
        }
        Some(_) => (evidence.rx_bytes, evidence.rx_units),
        None if evidence.rx_bytes != 0 || evidence.rx_units != 0 => {
            return Err(format!(
                "AP UDP flow {} unexpectedly received data",
                evidence.flow_id
            )
            .into());
        }
        None => (0, 0),
    };
    let (tx_bytes, tx_units, host_tx) = match host_rx {
        Some(bursts) => {
            if bursts.len() != 1 {
                return Err(format!(
                    "AP UDP flow {} produced {} TX bursts instead of one",
                    evidence.flow_id,
                    bursts.len(),
                )
                .into());
            }
            let host = bursts[0];
            if (exact_delivery && (!host.started_at_zero || host.missing != 0))
                || host.reordered != 0
                || host.duplicates != 0
            {
                return Err(format!(
                    "AP UDP flow {} TX ordering defect: started_at_zero={} missing={} reordered={} duplicates={}",
                    evidence.flow_id,
                    host.started_at_zero,
                    host.missing,
                    host.reordered,
                    host.duplicates,
                )
                .into());
            }
            if (exact_delivery
                && (host.bytes != evidence.tx_bytes || host.datagrams != evidence.tx_units))
                || (!exact_delivery
                    && (host.bytes > evidence.tx_bytes || host.datagrams > evidence.tx_units))
            {
                return Err(format!(
                    "AP UDP flow {} TX mismatch: target={}/{} host={}/{}",
                    evidence.flow_id,
                    evidence.tx_bytes,
                    evidence.tx_units,
                    host.bytes,
                    host.datagrams,
                )
                .into());
            }
            (host.bytes, host.datagrams, Some(host))
        }
        None if evidence.tx_bytes != 0 || evidence.tx_units != 0 => {
            return Err(format!(
                "AP UDP flow {} unexpectedly transmitted data",
                evidence.flow_id
            )
            .into());
        }
        None => (0, 0, None),
    };
    let bitrate = |bytes: u64| {
        bytes
            .saturating_mul(8_000_000)
            .checked_div(evidence.elapsed_micros.max(1))
            .unwrap_or(0)
    };
    Ok(MultiClientFlowReport {
        flow_id: evidence.flow_id,
        peer,
        rx_bytes,
        tx_bytes,
        rx_units,
        tx_units,
        elapsed_micros: evidence.elapsed_micros,
        rx_bps: bitrate(rx_bytes),
        tx_bps: bitrate(tx_bytes),
        host_tx_started_at_zero: host_tx.map(|host| host.started_at_zero),
        host_tx_missing: host_tx.map(|host| host.missing),
        host_tx_reordered: host_tx.map(|host| host.reordered),
        host_tx_duplicates: host_tx.map(|host| host.duplicates),
        host_tx_maximum_interarrival_us: host_tx.map(|host| host.maximum_interarrival_us),
        host_tx_sequence_after_maximum_interarrival: host_tx
            .and_then(|host| host.sequence_after_maximum_interarrival),
    })
}

pub(super) fn validate_multi_client_fairness(
    flows: &[MultiClientFlowReport; SESSION_FLOW_CAPACITY],
    direction: Direction,
    criteria: &MultiClientCriteria,
    measurements: &oer_hil_workload::measurements::Recorder,
) -> Result<()> {
    use oer_hil_run_bundle_format::run::Comparison;
    use oer_hil_run_bundle_format::run::Measurement;
    use oer_hil_run_bundle_format::run::MeasurementUnit;
    let validate = |label: &str, rates: [u64; SESSION_FLOW_CAPACITY]| -> Result<()> {
        let minimum = criteria.minimum_bps_per_flow;
        let name = |metric: &str| format!("ap.{}.{metric}", label.to_ascii_lowercase());
        let slowest = rates.into_iter().min().unwrap_or(0);
        let fastest = rates.into_iter().max().unwrap_or(0);
        measurements.rate(
            &name("flow-minimum-rate"),
            slowest,
            (minimum != 0).then_some(minimum),
        );
        // Rounded up, so the verdict agrees with the exact check below.
        let skew_basis_points = u64::try_from(
            u128::from(fastest - slowest)
                .saturating_mul(10_000)
                .checked_next_multiple_of(u128::from(fastest.max(1)))
                .map_or(u128::MAX, |scaled| scaled / u128::from(fastest.max(1))),
        )
        .unwrap_or(u64::MAX);
        let skew = Measurement::observed(
            name("flow-skew"),
            skew_basis_points,
            MeasurementUnit::BasisPoints,
        );
        measurements.record([match criteria.maximum_flow_skew_percent {
            Some(percent) => {
                skew.evaluated(Comparison::AtMost, u64::from(percent).saturating_mul(100))
            }
            None => skew,
        }]);
        for (index, rate) in rates.into_iter().enumerate() {
            if rate < minimum {
                return Err(format!(
                    "AP {label} flow {index} bitrate {rate} is below required {minimum}",
                )
                .into());
            }
        }
        if let Some(maximum_percent) = criteria.maximum_flow_skew_percent {
            let minimum = rates.into_iter().min().unwrap_or(0);
            let maximum = rates.into_iter().max().unwrap_or(0);
            if u128::from(maximum.saturating_sub(minimum)).saturating_mul(100)
                > u128::from(maximum).saturating_mul(u128::from(maximum_percent))
            {
                return Err(format!(
                    "AP {label} flow skew exceeds {maximum_percent}%: min={minimum} max={maximum}",
                )
                .into());
            }
        }
        Ok(())
    };
    if target_receives(direction) {
        validate("RX", flows.each_ref().map(|flow| flow.rx_bps))?;
    }
    if target_transmits(direction) {
        validate("TX", flows.each_ref().map(|flow| flow.tx_bps))?;
        if let Some(minimum) = criteria.minimum_secondary_tx_datagrams
            && flows[1].tx_units < u64::from(minimum)
        {
            return Err(format!(
                "AP secondary TX delivered {} datagrams; required {minimum}",
                flows[1].tx_units,
            )
            .into());
        }
        if let Some(maximum_ms) = criteria.maximum_secondary_tx_interarrival_ms {
            let secondary = flows[1];
            let maximum_us = secondary
                .host_tx_maximum_interarrival_us
                .ok_or("AP sparse secondary TX flow omitted host inter-arrival evidence")?;
            if maximum_us > u64::from(maximum_ms).saturating_mul(1_000) {
                return Err(format!(
                    "AP secondary TX inter-arrival {maximum_us} us exceeds {maximum_ms} ms after sequence {:?}",
                    secondary.host_tx_sequence_after_maximum_interarrival,
                )
                .into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
