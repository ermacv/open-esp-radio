//! Host receiver and typed result for the production UDP TX qualification.

mod progress;
mod receiver;
mod terminal;
pub use receiver::Receiver;

use crate::link::WifiCapture as _;
use oer_hil_net_traffic::NetworkSession as _;
use oer_hil_run_bundle_format::run::fixtures::Burst;
use oer_hil_workload::context::Context;
use std::{
    collections::HashSet,
    fs,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    path::Path,
    time::{Duration, Instant},
};

use oer_hil_protocol::{
    network::Completion, network::Direction, network::FlowConfig, network::Ipv4Endpoint,
    network::SessionConfig, network::SessionFlowConfig, network::SessionLinkRequirements,
    network::Transport,
};

use crate::{
    Result, workload::traffic::bidirectional::AmpduEvidence,
    workload::traffic::bidirectional::MIN_QUALIFIED_AGGREGATES,
    workload::traffic::bidirectional::TaskPollSet,
    workload::traffic::bidirectional::TxQualification,
    workload::traffic::bidirectional::post_block_ack_delivery_loss_lower_bound,
    workload::traffic::bidirectional::task_polls_from_log,
};
use oer_hil_family_ieee80211_fixture::{
    host_network::BenchmarkIpv4Route, local::evidence::LocalLinuxTxCapture,
    local::evidence::LocalLinuxTxEvidence, openwrt::evidence::ChannelUtilization,
    openwrt::evidence::OpenWrtStationLinkEvidence,
    openwrt::evidence::require_idle_channel_utilization, openwrt::evidence::station_link,
    station_fixture::require_ht40_mcs7,
};
use oer_hil_lab::config::StationFixtureConfig;
use oer_hil_net_traffic::{
    await_udp_tx_ready,
    udp::{configure_qualification_receive_buffer, confirm_reverse_flow},
};
use oer_hil_scenario_catalog::link::PhyExpectation;
use serde::Serialize;

const DEFAULT_PORT: u16 = 9_002;
const DEVICE_SOURCE_PORT: u16 = 4_324;
const DEFAULT_DURATION: Duration = Duration::from_secs(16);
const DEFAULT_PAYLOAD: usize = 1_472;
const MIN_BURST_DATAGRAMS: u64 = 1_000;
const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct Config {
    pub device: Ipv4Addr,
    pub port: u16,
    pub duration: Duration,
    pub payload: usize,
    pub offered_rate_bps: Option<u64>,
    pub throughput_floor_bps: Option<u64>,
    pub bandwidth_mhz: u16,
    pub minimum_rate_kbps: u64,
    pub maximum_idle_channel_utilization_255: Option<u8>,
    /// Suspend the shared PHY's periodic tracking for the session.
    pub suspend_phy_tracking: bool,
}

struct ActiveBurst {
    evidence: Burst,
    started: Instant,
    last: Instant,
    lowest_sequence: u32,
    highest_sequence: u32,
    seen_sequences: HashSet<u32>,
}

impl ActiveBurst {
    fn new(sequence: u32, length: usize, now: Instant) -> Self {
        let mut seen_sequences = HashSet::new();
        seen_sequences.insert(sequence);
        Self {
            evidence: Burst {
                bytes: length as u64,
                datagrams: 1,
                started_at_zero: sequence == 0,
                ..Burst::default()
            },
            started: now,
            last: now,
            lowest_sequence: sequence,
            highest_sequence: sequence,
            seen_sequences,
        }
    }

    fn push(&mut self, sequence: u32, length: usize, now: Instant) {
        let interarrival_us = now
            .duration_since(self.last)
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX);
        if interarrival_us > self.evidence.maximum_interarrival_us {
            self.evidence.maximum_interarrival_us = interarrival_us;
            self.evidence.sequence_after_maximum_interarrival = Some(sequence);
        }
        if !self.seen_sequences.insert(sequence) {
            self.evidence.duplicates = self.evidence.duplicates.saturating_add(1);
        } else if sequence < self.highest_sequence {
            self.evidence.reordered = self.evidence.reordered.saturating_add(1);
            if self.evidence.first_reordered_sequence.is_none() {
                self.evidence.first_reordered_after = Some(self.highest_sequence);
                self.evidence.first_reordered_sequence = Some(sequence);
            }
            self.evidence.maximum_reorder_distance = self
                .evidence
                .maximum_reorder_distance
                .max(self.highest_sequence - sequence);
        }
        self.lowest_sequence = self.lowest_sequence.min(sequence);
        self.highest_sequence = self.highest_sequence.max(sequence);
        self.evidence.bytes = self.evidence.bytes.saturating_add(length as u64);
        self.evidence.datagrams = self.evidence.datagrams.saturating_add(1);
        self.last = now;
    }

    fn finish(mut self) -> Burst {
        let sequence_span = u64::from(self.highest_sequence - self.lowest_sequence) + 1;
        self.evidence.missing = sequence_span.saturating_sub(self.seen_sequences.len() as u64);
        let mut active_missing_run_start = None;
        for sequence in self.lowest_sequence..=self.highest_sequence {
            if self.seen_sequences.contains(&sequence) {
                if let Some(start) = active_missing_run_start.take() {
                    self.record_missing_run(start, sequence - 1);
                }
            } else if active_missing_run_start.is_none() {
                active_missing_run_start = Some(sequence);
            }
        }
        if let Some(start) = active_missing_run_start {
            self.record_missing_run(start, self.highest_sequence);
        }
        self.evidence.elapsed_us = self
            .last
            .duration_since(self.started)
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX)
            .max(1);
        self.evidence.lowest_sequence = self.lowest_sequence;
        self.evidence.highest_sequence = self.highest_sequence;
        self.evidence
    }

    fn record_missing_run(&mut self, start: u32, end: u32) {
        let length = u64::from(end - start) + 1;
        self.evidence.missing_runs = self.evidence.missing_runs.saturating_add(1);
        if length > self.evidence.maximum_missing_run {
            self.evidence.maximum_missing_run = length;
            self.evidence.maximum_missing_run_start = Some(start);
            self.evidence.maximum_missing_run_end = Some(end);
        }
    }
}

pub fn run(
    options: Config,
    output: &Path,
    context: &Context<'_>,
    require_exact_delivery: bool,
    require_no_beacon_loss: bool,
    require_driver_observation: bool,
) -> Result<()> {
    let mut options = options.validate()?;
    fs::create_dir_all(output)?;
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, options.port))?;
    let host_receive_buffer_bytes = configure_qualification_receive_buffer(&socket)?;
    let capture = context.capture(output)?;
    let discovered_address = match await_udp_tx_ready(
        &capture,
        context.target(),
        options.device,
        DEVICE_READY_TIMEOUT,
    ) {
        Ok(address) => address,
        Err(error) => {
            return capture.finish_with(Err(error));
        }
    };
    options.device = discovered_address.address;
    let host_route =
        match BenchmarkIpv4Route::discover(options.device, &context.lab.station_fixture) {
            Ok(route) => route,
            Err(error) => {
                return capture.finish_with(Err(error));
            }
        };
    socket.connect(SocketAddrV4::new(options.device, DEVICE_SOURCE_PORT))?;
    let host_address = match socket.local_addr()? {
        SocketAddr::V4(address) => *address.ip(),
        SocketAddr::V6(_) => return Err("TX qualification requires IPv4".into()),
    };
    host_route.verify_socket_source(host_address)?;
    host_route.record(output, options.device, host_address)?;
    // Confirm the exact reverse flow and neighbor resolution before load.
    // The response is outside the measured session and uses this same socket.
    if let Err(error) = confirm_reverse_flow(&socket, Duration::from_secs(5)) {
        return capture.finish_with(Err(error));
    }
    let pre_workload_channel_utilization = match (
        options.maximum_idle_channel_utilization_255,
        &context.lab.station_fixture,
    ) {
        (Some(maximum), StationFixtureConfig::OpenWrt(config)) => {
            match require_idle_channel_utilization(config, maximum) {
                Ok(utilization) => Some(utilization),
                Err(error) => {
                    return capture.finish_with(Err(error));
                }
            }
        }
        (Some(_), StationFixtureConfig::LocalLinux(_) | StationFixtureConfig::External(_)) => {
            return capture.finish_with(Err(
                "TX idle-channel evidence requires a managed OpenWrt fixture".into(),
            ));
        }
        (None, _) => None,
    };
    let local_ingress_capture = match &context.lab.station_fixture {
        StationFixtureConfig::LocalLinux(config) => Some(LocalLinuxTxCapture::start(
            config,
            options.device,
            DEVICE_SOURCE_PORT,
            options.port,
            options.duration,
            if options.bandwidth_mhz == 40 {
                PhyExpectation::Ht40
            } else {
                PhyExpectation::He20
            },
        )?),
        StationFixtureConfig::OpenWrt(_) | StationFixtureConfig::External(_) => None,
    };

    let timeout = options.duration.saturating_add(Duration::from_secs(5));
    let receiver = Receiver::start(
        &socket,
        options.device,
        timeout + oer_hil_net_traffic::SESSION_START_TIMEOUT,
        output,
        "station",
    )?;
    let tracking = match super::phy_tracking::begin(&capture, options.suspend_phy_tracking) {
        Ok(tracking) => tracking,
        Err(error) => return capture.finish_with(Err(error)),
    };
    let session = match capture.start_session(SessionConfig {
        network_interface: oer_hil_protocol::wifi::WifiNetworkInterface::Station,
        transport: Transport::Udp,
        direction: Direction::Tx,
        completion: Completion::DurationMillis(u32::try_from(options.duration.as_millis())?),
        flows: [
            Some(SessionFlowConfig {
                flow_id: 0,
                peer: Some(Ipv4Endpoint {
                    address: host_address.octets(),
                    port: options.port,
                }),
                target_rx: None,
                target_tx: Some(FlowConfig {
                    payload_bytes: u16::try_from(options.payload)?,
                    offered_rate_bps: options.offered_rate_bps,
                    pacing_group_datagrams: None,
                }),
                payload_identity: None,
            }),
            None,
        ],
        link_requirements: SessionLinkRequirements::tx_block_ack(0),
    }) {
        Ok(session) => session,
        Err(error) => {
            return capture.finish_with(Err(error));
        }
    };
    let structured = capture.wait_for_session(session, timeout);
    if let Ok(evidence) = &structured
        && let Some(load) = &evidence.cpu_load
    {
        context.measurements.cpu_load(load);
    }
    let bursts = receiver.finish(
        structured
            .as_ref()
            .ok()
            .map(|evidence| evidence.transport.tx_units),
    );
    let structured = match structured {
        Ok(evidence) => evidence,
        Err(error) => return capture.finish_with(Err(error)),
    };
    let bursts = bursts?;
    if let Some(tx) = structured
        .radio
        .as_ref()
        .and_then(|radio| radio.tx.as_ref())
    {
        terminal::retain(&context.results, tx.station_terminal)?;
    }

    if let Err(error) = capture.acknowledge_session(session) {
        return capture.finish_with(Err(error));
    }
    if let Err(error) = super::phy_tracking::finish(&capture, &context.results, tracking) {
        return capture.finish_with(Err(error));
    }
    let local_ingress = local_ingress_capture
        .map(LocalLinuxTxCapture::finish)
        .transpose()?;
    let openwrt_link = match &context.lab.station_fixture {
        StationFixtureConfig::OpenWrt(config) => Some(station_link(config, options.device)?),
        StationFixtureConfig::LocalLinux(_) | StationFixtureConfig::External(_) => None,
    };
    if let Some(evidence) = local_ingress.as_ref() {
        context.results.observe("local-wireless-ingress", evidence);
    }
    let beacon_loss = require_no_beacon_loss.then(|| capture.require_no_beacon_loss());
    let log = capture.finish()?;
    context
        .measurements
        .record(task_polls_from_log(&log).measurements());
    let delivery = progress::DeliveryProgress::new(structured.transport.tx_units, &bursts, &log);
    context.results.observe("delivery-progress", &delivery);
    if delivery.host_received_datagrams == 0 {
        return Err(delivery.no_delivery_message().into());
    }
    if let Some(result) = beacon_loss {
        result?;
    }

    let qualified: Vec<_> = bursts
        .iter()
        .copied()
        .filter(|burst| burst.started_at_zero && burst.datagrams >= MIN_BURST_DATAGRAMS)
        .collect();
    let minimum_bursts = 1;
    if qualified.len() < minimum_bursts {
        return Err(format!(
            "received only {} complete TX bursts; required {minimum_bursts}; {}",
            qualified.len(),
            describe_bursts(&bursts),
        )
        .into());
    }
    let missing: u64 = qualified.iter().map(|burst| burst.missing).sum();
    let reordered: u64 = qualified.iter().map(|burst| burst.reordered).sum();
    let duplicates: u64 = qualified.iter().map(|burst| burst.duplicates).sum();
    let device_floor_kbps = structured
        .transport
        .tx_bytes
        .saturating_mul(8)
        .saturating_mul(1_000)
        .checked_div(structured.transport.elapsed_micros.max(1))
        .unwrap_or(0);
    let host_floor = qualified
        .iter()
        .map(|burst| burst.throughput_kbps())
        .min()
        .expect("at least one qualified burst");
    context.measurements.rate(
        "udp.tx.host-rate",
        host_floor.saturating_mul(1_000),
        options.throughput_floor_bps,
    );
    context.measurements.rate(
        "udp.tx.target-rate",
        device_floor_kbps.saturating_mul(1_000),
        options.throughput_floor_bps,
    );
    let link_report = require_performance_link(
        &context.lab.station_fixture,
        options.bandwidth_mhz,
        local_ingress.as_ref(),
        openwrt_link.as_ref(),
    )?;
    if !require_driver_observation {
        if structured.radio.is_some()
            || structured.tx_timing.is_some()
            || structured.rx_delivery.is_some()
            || structured.network_scheduler.is_some()
        {
            return Err("performance image published driver-internal evidence".into());
        }
        if !structured.finished.summary.verdict.passed() {
            return Err(format!(
                "target did not complete the typed TX session normally: {}",
                structured.finished.summary.verdict
            )
            .into());
        }
        if structured.transport.rx_bytes != 0 || structured.transport.rx_units != 0 {
            return Err("TX-only session reported unexpected received traffic".into());
        }
        if structured.transport.transport_errors != 0 {
            return Err(format!(
                "typed TX session reported {} transport errors",
                structured.transport.transport_errors
            )
            .into());
        }
        let throughput_failure = if let Some(required) = options.throughput_floor_bps {
            let measured_host = host_floor.saturating_mul(1_000);
            let measured_target = device_floor_kbps.saturating_mul(1_000);
            if measured_host < required || measured_target < required {
                Some(format!(
                    "TX throughput is below the configured floor: required={required} host={measured_host} target={measured_target} bit/s"
                ))
            } else {
                None
            }
        } else {
            None
        };
        context.results.observe(
            "tx",
            &TxPerformanceObservation {
                options: &options,
                host_address,
                bursts: &qualified,
                host_floor_kbps: host_floor,
                device_floor_kbps,
                structured,
                host_receive_buffer_bytes,
                link_report: &link_report,
                pre_workload_channel_utilization,
                task_polls: task_polls_from_log(&log),
                core0_coarse: Core0CoarseEvidence::from_log(&log),
                tx_phases: TxPhaseEvidence::from_log(&log),
                failure: throughput_failure.as_deref(),
            },
        );
        if let Some(failure) = throughput_failure {
            return Err(failure.into());
        }
        eprintln!(
            "OPENRADIOHOST result=PASS mode=tx-performance host_floor_kbps={host_floor} device_floor_kbps={device_floor_kbps} bursts={} host_receive_buffer_bytes={host_receive_buffer_bytes}",
            qualified.len(),
        );
        return Ok(());
    }
    let (typed_tx, typed_timing) = structured.require_tx_radio(
        options.bandwidth_mhz,
        options.minimum_rate_kbps,
        u32::try_from(MIN_QUALIFIED_AGGREGATES).unwrap_or(u32::MAX),
    )?;
    let tx = TxQualification {
        throughput_floor_kbps: device_floor_kbps,
        sample_count: 1,
        ampdu: AmpduEvidence::from_typed(typed_tx, typed_timing),
    };
    if require_exact_delivery && (missing != 0 || reordered != 0 || duplicates != 0) {
        let received_datagrams = qualified.iter().map(|burst| burst.datagrams).sum::<u64>();
        let lowest_sequence = qualified
            .iter()
            .map(|burst| burst.lowest_sequence)
            .min()
            .unwrap_or(0);
        let highest_sequence = qualified
            .iter()
            .map(|burst| burst.highest_sequence)
            .max()
            .unwrap_or(0);
        let maximum_interarrival = qualified
            .iter()
            .max_by_key(|burst| burst.maximum_interarrival_us)
            .copied()
            .unwrap_or_default();
        let target_tx_units = Some(structured.transport.tx_units);
        let host_delivery = Burst {
            datagrams: received_datagrams,
            duplicates,
            ..Burst::default()
        };
        let post_block_ack_loss = target_tx_units.and_then(|units| {
            post_block_ack_delivery_loss_lower_bound(tx.ampdu, host_delivery, units)
        });
        return Err(format!(
            "host observed TX sequence defects: missing={missing} reordered={reordered} \
             duplicates={duplicates} received={received_datagrams} \
             range={lowest_sequence}..={highest_sequence} max_interarrival_us={} \
             sequence_after_max_interarrival={:?} missing_runs={} \
             maximum_missing_run={} maximum_missing_range={:?}..={:?} \
             target_tx_units={target_tx_units:?} ampdu_subframes={} block_acknowledged={} \
             post_block_ack_delivery_loss_lower_bound={post_block_ack_loss:?} \
             local_wireless_ingress={:?}",
            maximum_interarrival.maximum_interarrival_us,
            maximum_interarrival.sequence_after_maximum_interarrival,
            qualified
                .iter()
                .map(|burst| burst.missing_runs)
                .sum::<u64>(),
            qualified
                .iter()
                .map(|burst| burst.maximum_missing_run)
                .max()
                .unwrap_or(0),
            qualified
                .iter()
                .max_by_key(|burst| burst.maximum_missing_run)
                .and_then(|burst| burst.maximum_missing_run_start),
            qualified
                .iter()
                .max_by_key(|burst| burst.maximum_missing_run)
                .and_then(|burst| burst.maximum_missing_run_end),
            tx.ampdu.subframes,
            tx.ampdu.acknowledged,
            local_ingress.as_ref().map(|evidence| evidence.udp_packets),
        )
        .into());
    }
    if let Some(required) = options.throughput_floor_bps {
        let measured_host = host_floor.saturating_mul(1_000);
        let measured_target = tx.throughput_floor_kbps.saturating_mul(1_000);
        if measured_host < required || measured_target < required {
            return Err(format!(
                "TX throughput is below the configured floor: required={required} host={measured_host} target={measured_target} bit/s"
            )
            .into());
        }
    }
    {
        let evidence = structured;
        let received_bytes = qualified.iter().map(|burst| burst.bytes).sum::<u64>();
        let received_datagrams = qualified.iter().map(|burst| burst.datagrams).sum::<u64>();
        if !evidence.finished.summary.verdict.passed() {
            return Err(format!(
                "target did not complete the typed TX session normally: {}",
                evidence.finished.summary.verdict
            )
            .into());
        }
        if evidence.transport.rx_bytes != 0 || evidence.transport.rx_units != 0 {
            return Err("TX-only session reported unexpected received traffic".into());
        }
        if evidence.transport.transport_errors != 0 {
            return Err(format!(
                "typed TX session reported {} transport errors",
                evidence.transport.transport_errors
            )
            .into());
        }
        if require_exact_delivery
            && let Some(local) = local_ingress.as_ref()
            && local.udp_packets != evidence.transport.tx_units
        {
            return Err(format!(
                "target/local wireless TX delivery mismatch: target={} local_ingress={}",
                evidence.transport.tx_units, local.udp_packets
            )
            .into());
        }
        if require_exact_delivery
            && (evidence.transport.tx_bytes != received_bytes
                || evidence.transport.tx_units != received_datagrams)
        {
            return Err(format!(
                "typed/host TX delivery mismatch: target={}/{} host={received_bytes}/{received_datagrams} \
                 range={}..={} max_interarrival_us={} sequence_after_max_interarrival={:?}",
                evidence.transport.tx_bytes,
                evidence.transport.tx_units,
                qualified[0].lowest_sequence,
                qualified[0].highest_sequence,
                qualified[0].maximum_interarrival_us,
                qualified[0].sequence_after_maximum_interarrival,
            )
            .into());
        }
    }
    context.results.observe(
        "tx",
        &TxObservation {
            options: &options,
            host_address,
            bursts: &qualified,
            host_floor_kbps: host_floor,
            device_floor_kbps: tx.throughput_floor_kbps,
            device_samples: tx.sample_count,
            ampdu: tx.ampdu,
            structured,
            host_receive_buffer_bytes,
            require_exact_delivery,
            link_report: &link_report,
            pre_workload_channel_utilization,
            task_polls: task_polls_from_log(&log),
        },
    );
    eprintln!(
        "OPENRADIOHOST result=PASS mode=tx host_floor_kbps={host_floor} \
         device_floor_kbps={} bursts={} missing={missing} reordered={reordered} duplicates={duplicates} \
         host_receive_buffer_bytes={} ampdu_avg_subframes={:.2} ampdu_31={} ampdu_32={}",
        tx.throughput_floor_kbps,
        qualified.len(),
        host_receive_buffer_bytes,
        tx.ampdu.subframes as f64 / tx.ampdu.aggregates.max(1) as f64,
        tx.ampdu.thirtyone,
        tx.ampdu.full32,
    );
    Ok(())
}

fn require_performance_link(
    fixture: &StationFixtureConfig,
    bandwidth_mhz: u16,
    local: Option<&LocalLinuxTxEvidence>,
    openwrt: Option<&OpenWrtStationLinkEvidence>,
) -> Result<String> {
    match fixture {
        StationFixtureConfig::LocalLinux(_) => {
            let evidence = local.ok_or("local AP link snapshot is unavailable")?;
            if bandwidth_mhz == 40 {
                require_ht40_mcs7(
                    "STA TX/AP RX",
                    evidence.channel_width_mhz,
                    &evidence.rx_bitrate,
                )?;
            }
            Ok(format!(
                "local AP interface width={} MHz; AP TX/RX bitrate=`{}` / `{}`",
                evidence.channel_width_mhz, evidence.tx_bitrate, evidence.rx_bitrate
            ))
        }
        StationFixtureConfig::OpenWrt(_) => {
            let evidence = openwrt.ok_or("OpenWrt AP link snapshot is unavailable")?;
            if bandwidth_mhz == 40 {
                require_ht40_mcs7(
                    "STA TX/AP RX",
                    evidence.channel_width_mhz,
                    &evidence.rx_bitrate,
                )?;
            }
            Ok(format!(
                "OpenWrt AP interface width={} MHz; AP TX/RX bitrate=`{}` / `{}`",
                evidence.channel_width_mhz, evidence.tx_bitrate, evidence.rx_bitrate
            ))
        }
        StationFixtureConfig::External(_) if bandwidth_mhz == 40 => {
            Err("HT40 performance requires a managed fixture link snapshot".into())
        }
        StationFixtureConfig::External(_) => {
            Ok(String::from("external AP; link vector not observed"))
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device: Ipv4Addr::UNSPECIFIED,
            port: DEFAULT_PORT,
            duration: DEFAULT_DURATION,
            payload: DEFAULT_PAYLOAD,
            offered_rate_bps: None,
            throughput_floor_bps: None,

            bandwidth_mhz: 20,
            minimum_rate_kbps: 114_700,
            maximum_idle_channel_utilization_255: None,
            suspend_phy_tracking: false,
        }
    }
}
impl Config {
    fn validate(self) -> Result<Self> {
        if !(Duration::from_secs(8)..=Duration::from_secs(300)).contains(&self.duration) {
            return Err("traffic duration must be in 8..=300 seconds".into());
        }
        if !(64..=1472).contains(&self.payload) {
            return Err("UDP payload must be in 64..=1472 bytes".into());
        }
        if self.maximum_idle_channel_utilization_255 == Some(0) {
            return Err("maximum idle channel utilization must be nonzero".into());
        }
        if [self.offered_rate_bps, self.throughput_floor_bps]
            .into_iter()
            .flatten()
            .any(|rate| !(100_000..=1_000_000_000).contains(&rate))
        {
            return Err("traffic rate is outside the supported range".into());
        }
        if self.port == 0 {
            return Err("port must be nonzero".into());
        }

        Ok(self)
    }
}

pub fn describe_bursts(bursts: &[Burst]) -> String {
    let datagrams = bursts.iter().map(|burst| burst.datagrams).sum::<u64>();
    let zero_started = bursts.iter().filter(|burst| burst.started_at_zero).count();
    let lowest = bursts.iter().map(|burst| burst.lowest_sequence).min();
    let highest = bursts.iter().map(|burst| burst.highest_sequence).max();
    format!(
        "observed_bursts={} observed_datagrams={datagrams} zero_started={zero_started} sequence_range={lowest:?}..={highest:?}",
        bursts.len(),
    )
}

/// The typed result of one TX performance session: the host sink's bursts,
/// the target's session evidence, the AP link description and the optional
/// Core0 and TX phase counters the diagnostic image prints.
#[derive(Serialize)]
struct TxPerformanceObservation<'a> {
    options: &'a Config,
    host_address: Ipv4Addr,
    bursts: &'a [Burst],
    host_floor_kbps: u64,
    device_floor_kbps: u64,
    structured: oer_hil_net_traffic::SessionEvidence,
    host_receive_buffer_bytes: usize,
    link_report: &'a str,
    pre_workload_channel_utilization: Option<ChannelUtilization>,
    task_polls: TaskPollSet,
    core0_coarse: Option<Core0CoarseEvidence>,
    tx_phases: Option<TxPhaseEvidence>,
    failure: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
struct Core0CoarseEvidence {
    radio_polls: u64,
    radio_cycles: u64,
    radio_instructions: u64,
}

impl Core0CoarseEvidence {
    fn from_log(log: &str) -> Option<Self> {
        let line = log
            .lines()
            .find(|line| line.starts_with("ORC0C ") || line.contains(" ORC0C "))?;
        Some(Self {
            radio_polls: numeric_field(line, "radio_polls")?,
            radio_cycles: numeric_field(line, "radio_cycles")?,
            radio_instructions: numeric_field(line, "radio_instret")?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
struct TxPhaseEvidence {
    core0_start_calls: u64,
    core0_start_cycles: u64,
    core0_start_instructions: u64,
    core0_prepare_calls: u64,
    core0_prepare_cycles: u64,
    core0_prepare_instructions: u64,
    core0_publish_calls: u64,
    core0_publish_cycles: u64,
    core0_publish_instructions: u64,
    core0_service_calls: u64,
    core0_service_cycles: u64,
    core0_service_instructions: u64,
    core0_encode_calls: u64,
    core0_encode_cycles: u64,
    core0_encode_instructions: u64,
    core0_commit_calls: u64,
    core0_commit_cycles: u64,
    core0_commit_instructions: u64,
    core1_admission_attempts: u64,
    core1_admission_successes: u64,
    core1_admission_cycles: u64,
    core1_admission_instructions: u64,
    core1_consume_calls: u64,
    core1_consume_bytes: u64,
    core1_consume_cycles: u64,
    core1_consume_instructions: u64,
    core1_emit_cycles: u64,
    core1_emit_instructions: u64,
    core1_publication_cycles: u64,
    core1_publication_instructions: u64,
}

impl TxPhaseEvidence {
    fn from_log(log: &str) -> Option<Self> {
        let core0 = log
            .lines()
            .find(|line| line.starts_with("ORC0TX ") || line.contains(" ORC0TX "))?;
        let core1 = log
            .lines()
            .find(|line| line.starts_with("ONTX ") || line.contains(" ONTX "))?;
        let core0_nested = log
            .lines()
            .find(|line| line.starts_with("ORC0TXN ") || line.contains(" ORC0TXN "))?;
        Some(Self {
            core0_start_calls: numeric_field(core0, "start_calls")?,
            core0_start_cycles: numeric_field(core0, "start_cycles")?,
            core0_start_instructions: numeric_field(core0, "start_instret")?,
            core0_prepare_calls: numeric_field(core0, "prepare_calls")?,
            core0_prepare_cycles: numeric_field(core0, "prepare_cycles")?,
            core0_prepare_instructions: numeric_field(core0, "prepare_instret")?,
            core0_publish_calls: numeric_field(core0, "publish_calls")?,
            core0_publish_cycles: numeric_field(core0, "publish_cycles")?,
            core0_publish_instructions: numeric_field(core0, "publish_instret")?,
            core0_service_calls: numeric_field(core0, "service_calls")?,
            core0_service_cycles: numeric_field(core0, "service_cycles")?,
            core0_service_instructions: numeric_field(core0, "service_instret")?,
            core0_encode_calls: numeric_field(core0_nested, "encode_calls")?,
            core0_encode_cycles: numeric_field(core0_nested, "encode_cycles")?,
            core0_encode_instructions: numeric_field(core0_nested, "encode_instret")?,
            core0_commit_calls: numeric_field(core0_nested, "commit_calls")?,
            core0_commit_cycles: numeric_field(core0_nested, "commit_cycles")?,
            core0_commit_instructions: numeric_field(core0_nested, "commit_instret")?,
            core1_admission_attempts: numeric_field(core1, "admission_attempts")?,
            core1_admission_successes: numeric_field(core1, "admission_successes")?,
            core1_admission_cycles: numeric_field(core1, "admission_cycles")?,
            core1_admission_instructions: numeric_field(core1, "admission_instret")?,
            core1_consume_calls: numeric_field(core1, "consume_calls")?,
            core1_consume_bytes: numeric_field(core1, "consume_bytes")?,
            core1_consume_cycles: numeric_field(core1, "consume_cycles")?,
            core1_consume_instructions: numeric_field(core1, "consume_instret")?,
            core1_emit_cycles: numeric_field(core1, "emit_cycles")?,
            core1_emit_instructions: numeric_field(core1, "emit_instret")?,
            core1_publication_cycles: numeric_field(core1, "publication_cycles")?,
            core1_publication_instructions: numeric_field(core1, "publication_instret")?,
        })
    }
}

fn numeric_field(line: &str, key: &str) -> Option<u64> {
    line.split_ascii_whitespace().find_map(|token| {
        let (candidate, value) = token.split_once('=')?;
        (candidate == key).then(|| value.parse().ok()).flatten()
    })
}

/// The typed result of one qualified TX session.
#[derive(Serialize)]
struct TxObservation<'a> {
    options: &'a Config,
    host_address: Ipv4Addr,
    bursts: &'a [Burst],
    host_floor_kbps: u64,
    device_floor_kbps: u64,
    device_samples: usize,
    ampdu: AmpduEvidence,
    structured: oer_hil_net_traffic::SessionEvidence,
    host_receive_buffer_bytes: usize,
    require_exact_delivery: bool,
    link_report: &'a str,
    pre_workload_channel_utilization: Option<ChannelUtilization>,
    task_polls: TaskPollSet,
}

#[cfg(test)]
mod tests;
