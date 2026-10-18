//! TCP workload execution and exact-delivery assessment.

use oer_hil_net_traffic::NetworkSession as _;
use std::{net::Ipv4Addr, time::Duration};

use oer_hil_protocol::{
    network::Completion, network::FlowConfig, network::SessionConfig, network::SessionFlowConfig,
    network::SessionLinkRequirements, network::Transport,
};

use crate::Result;
use crate::scenario::{Direction, RateFloors};
use crate::workload::ieee80211::access_point::{
    Config, TCP_PORT, protocol_direction, report::TrafficReport, session_report,
    validate_rate_criteria,
};
use oer_hil_link::SerialCapture;
use oer_hil_net_traffic::SessionEvidence;
use oer_hil_net_traffic::{
    paced_tcp::Config as TcpConfig, paced_tcp::HostReception as TcpReception,
    paced_tcp::HostTransmission as TcpTransmission, paced_tcp::exchange as exchange_tcp,
    paced_tcp::receive as receive_tcp, paced_tcp::send as send_tcp,
};

#[derive(Clone, Copy)]
pub(super) struct TcpWorkload {
    pub(super) direction: Direction,
    pub(super) duration: Duration,
    pub(super) rx_rate_bps: Option<u64>,
    pub(super) tx_rate_bps: Option<u64>,
    pub(super) chunk_bytes: usize,
}

pub(super) fn qualify_tcp(
    capture: &SerialCapture,
    config: &Config,
    measurements: &oer_hil_workload::measurements::Recorder,
    target: Ipv4Addr,
    workload: TcpWorkload,
    floors: RateFloors,
) -> Result<TrafficReport> {
    let TcpWorkload {
        direction,
        duration,
        rx_rate_bps,
        tx_rate_bps,
        chunk_bytes,
    } = workload;
    let protocol_direction = protocol_direction(direction);
    let duration_millis = u32::try_from(duration.as_millis())?;
    let session = capture.start_session(SessionConfig {
        network_interface: oer_hil_protocol::wifi::WifiNetworkInterface::AccessPoint,
        transport: Transport::Tcp,
        direction: protocol_direction,
        completion: Completion::DurationMillis(duration_millis),
        flows: [
            Some(SessionFlowConfig {
                flow_id: 0,
                peer: None,
                target_rx: rx_rate_bps.map(|rate| FlowConfig {
                    payload_bytes: u16::try_from(chunk_bytes).expect("validated TCP chunk"),
                    offered_rate_bps: Some(rate),
                    pacing_group_datagrams: None,
                }),
                target_tx: tx_rate_bps.map(|rate| FlowConfig {
                    payload_bytes: u16::try_from(chunk_bytes).expect("validated TCP chunk"),
                    offered_rate_bps: Some(rate),
                    pacing_group_datagrams: None,
                }),
                payload_identity: None,
            }),
            None,
        ],
        link_requirements: SessionLinkRequirements::NONE,
    })?;
    let tcp = TcpConfig {
        address: target,
        port: TCP_PORT,
        rate_bps: rx_rate_bps
            .or(tx_rate_bps)
            .expect("validated AP TCP direction has a rate"),
        duration,
        chunk_bytes,
    };
    let data_plane = match direction {
        Direction::Rx => send_tcp(tcp).map(|sample| (Some(sample), None)),
        Direction::Tx => receive_tcp(tcp).map(|sample| (None, Some(sample))),
        Direction::Bidirectional => {
            exchange_tcp(tcp).map(|(sent, received)| (Some(sent), Some(received)))
        }
    };
    let structured = capture.wait_for_session(session, config.timeout);
    if let Ok(evidence) = &structured
        && let Some(load) = &evidence.cpu_load
    {
        measurements.cpu_load(load);
    }
    let acknowledgement = structured
        .as_ref()
        .map(|_| capture.acknowledge_session(session))
        .unwrap_or(Ok(()));
    let (host_tx, host_rx) =
        data_plane.map_err(|error| format!("AP TCP host path failed: {error}"))?;
    let structured = structured.map_err(|error| format!("AP TCP target failed: {error}"))?;
    acknowledgement?;
    let report = session_report(direction, &structured);
    validate_tcp(host_tx, host_rx, structured)?;
    validate_rate_criteria(&report, floors, measurements)?;
    Ok(TrafficReport::Tcp(report))
}

pub(super) fn validate_tcp(
    host_tx: Option<TcpTransmission>,
    host_rx: Option<TcpReception>,
    evidence: SessionEvidence,
) -> Result<()> {
    if !evidence.finished.summary.verdict.passed() || evidence.transport.transport_errors != 0 {
        return Err(format!(
            "AP TCP target failed: verdict={} errors={}",
            evidence.finished.summary.verdict, evidence.transport.transport_errors,
        )
        .into());
    }
    match host_tx {
        Some(host)
            if host.bytes != evidence.transport.rx_bytes || evidence.transport.rx_units != 1 =>
        {
            return Err(format!(
                "AP TCP RX mismatch: host={} target={} units={}",
                host.bytes, evidence.transport.rx_bytes, evidence.transport.rx_units,
            )
            .into());
        }
        None if evidence.transport.rx_bytes != 0 || evidence.transport.rx_units != 0 => {
            return Err("AP TCP TX-only session reported received traffic".into());
        }
        _ => {}
    }
    match host_rx {
        Some(host)
            if host.bytes != evidence.transport.tx_bytes
                || evidence.transport.tx_units != 1
                || !host.eof
                || host.pattern_errors != 0 =>
        {
            return Err(format!(
                "AP TCP TX mismatch: target={} host={} units={} eof={} pattern_errors={}",
                evidence.transport.tx_bytes,
                host.bytes,
                evidence.transport.tx_units,
                host.eof,
                host.pattern_errors,
            )
            .into());
        }
        None if evidence.transport.tx_bytes != 0 || evidence.transport.tx_units != 0 => {
            return Err("AP TCP RX-only session reported transmitted traffic".into());
        }
        _ => {}
    }
    Ok(())
}
