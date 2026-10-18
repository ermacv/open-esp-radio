//! Host side of the simultaneous RX/TX qualification cell.
//!
//! The firmware's `bidirectional` image owns a synthetic A-MPDU uplink while
//! this runner offers a paced UDP downlink.  Qualification is deliberately
//! based on device-side RX, TX-vector, placement and DMA-health evidence; a
//! successful host `send` alone is not evidence that the radio received it.

use crate::link::WifiCapture as _;
use oer_hil_net_traffic::NetworkSession as _;
use oer_hil_run_bundle_format::run::fixtures::Burst;
use oer_hil_workload::context::Context;
use std::{
    collections::BTreeMap,
    fs,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    path::Path,
    time::Duration,
};

use oer_hil_protocol::{
    network::Completion, network::Direction, network::FlowConfig, network::Ipv4Endpoint,
    network::RxRadioEvidence, network::SessionConfig, network::SessionFlowConfig,
    network::SessionLinkRequirements, network::Transport, network::TransportEvidence,
    network::TxAggregateTimingEvidence, network::TxRadioEvidence,
};

use crate::{
    Result, workload::traffic::tx_traffic::Receiver, workload::traffic::tx_traffic::describe_bursts,
};
use oer_hil_family_ieee80211_evidence as evidence;
use oer_hil_family_ieee80211_fixture::{
    host_network::BenchmarkIpv4Route, local::air_monitor::LocalAirMonitorCapture,
    local::air_monitor::LocalAirMonitorEvidence, local::evidence::LocalLinuxTxCapture,
    local::evidence::LocalLinuxTxEvidence, openwrt::tx_monitor::MacFrameKey,
    openwrt::tx_monitor::OpenWrtTxMonitorCapture, openwrt::tx_monitor::OpenWrtTxMonitorEvidence,
    station_fixture::RxCapture, station_fixture::RxEvidence,
};
use oer_hil_lab::config::StationFixtureConfig;
use oer_hil_net_traffic::{
    SessionEvidence, await_udp_rx_ready,
    udp::{configure_qualification_receive_buffer, confirm_reverse_flow},
};
use oer_hil_net_traffic::{
    paced_udp::Config as PacedUdpConfig, paced_udp::HostTransmission,
    paced_udp::send as send_paced_udp,
};
use oer_hil_scenario_catalog::link::HtGuardIntervalExpectation;
use serde::Serialize;

const DEFAULT_PORT: u16 = 4_323;
const DEFAULT_RATE_BPS: u64 = 10_000_000;
const DEFAULT_DURATION: Duration = Duration::from_secs(12);
const DEFAULT_PAYLOAD: usize = 1_200;
const DEFAULT_TX_PORT: u16 = 9_002;
const DEFAULT_TX_PAYLOAD: usize = 1_472;
const DEVICE_TX_SOURCE_PORT: u16 = 4_324;
const MIN_HOST_TX_DATAGRAMS: u64 = 1_000;
const MIN_QUALIFIED_SAMPLE: Duration = Duration::from_secs(4);
pub const MIN_QUALIFIED_AGGREGATES: u64 = 100;
const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(45);
const PSRAM_CODE_START: u64 = 0x5000_0000;
const PSRAM_CODE_END: u64 = 0x5100_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phy {
    Ht40,
    He20,
}

impl Phy {
    const fn name(self) -> &'static str {
        match self {
            Self::Ht40 => "ht40",
            Self::He20 => "he20",
        }
    }

    const fn required_tx(self) -> (u16, u64) {
        match self {
            // HT40 MCS7 is 135 Mbit/s with the mandatory long guard interval
            // and 150 Mbit/s when the peer-qualified short GI is selected.
            Self::Ht40 => (40, 135_000),
            Self::He20 => (20, 114_700),
        }
    }

    const fn expected_rx_format(self) -> u8 {
        match self {
            Self::Ht40 => 2,
            Self::He20 => 4,
        }
    }
}

#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct Config {
    pub maximum_rx_silence_ms: Option<u32>,
    pub address: Ipv4Addr,
    pub port: u16,
    pub rate_bps: u64,
    pub rx_floor_bps: Option<u64>,
    pub duration: Duration,
    pub payload: usize,
    pub tx_port: u16,
    pub tx_payload: usize,
    pub tx_rate_bps: Option<u64>,
    pub tx_floor_bps: Option<u64>,
    pub combined_floor_bps: Option<u64>,
    pub phy: Phy,
}

#[derive(Debug, Serialize)]
struct BidirectionalEvidence {
    host_offer: HostTransmission,
    target_rx: RxQualification,
    target_tx_floor_kbps: u64,
    host_sink: Burst,
    session: SessionEvidence,
    ampdu: AmpduEvidence,
    host_receive_buffer_bytes: usize,
    fixture_rx: Option<RxEvidence>,
    fixture_tx: Option<LocalLinuxTxEvidence>,
    tx_monitor_rx: Option<OpenWrtTxMonitorEvidence>,
    independent_air_rx: Option<LocalAirMonitorEvidence>,
}

pub struct RunPolicy {
    pub require_exact_delivery: bool,
    pub require_no_beacon_loss: bool,
    pub capture_openwrt_tx_monitor_rx: bool,
    pub capture_independent_laptop_air_monitor: bool,
    pub require_driver_observation: bool,
    pub minimum_mcs: Option<u8>,
    pub guard_interval: HtGuardIntervalExpectation,
    pub fixture_guard_interval: HtGuardIntervalExpectation,
}

pub fn run(options: Config, output: &Path, context: &Context<'_>, policy: RunPolicy) -> Result<()> {
    let RunPolicy {
        require_exact_delivery,
        require_no_beacon_loss,
        capture_openwrt_tx_monitor_rx,
        capture_independent_laptop_air_monitor,
        require_driver_observation,
        minimum_mcs,
        guard_interval,
        fixture_guard_interval,
    } = policy;
    let mut options = options.validate()?;
    fs::create_dir_all(output)?;
    let tx_sink = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, options.tx_port))?;
    let host_receive_buffer_bytes = configure_qualification_receive_buffer(&tx_sink)?;
    let capture = context.capture(output)?;
    let discovered_address = match await_udp_rx_ready(
        &capture,
        context.target(),
        options.address,
        options.port,
        DEVICE_READY_TIMEOUT,
    ) {
        Ok(address) => address,
        Err(error) => {
            return capture.finish_with(Err(error));
        }
    };
    options.address = discovered_address.address;
    let host_route =
        match BenchmarkIpv4Route::discover(options.address, &context.lab.station_fixture) {
            Ok(route) => route,
            Err(error) => {
                return capture.finish_with(Err(error));
            }
        };
    tx_sink.connect(SocketAddrV4::new(options.address, DEVICE_TX_SOURCE_PORT))?;
    let host_address = match tx_sink.local_addr()? {
        SocketAddr::V4(address) => *address.ip(),
        SocketAddr::V6(_) => return Err("bidirectional qualification requires IPv4".into()),
    };
    host_route.verify_socket_source(host_address)?;
    host_route.record(output, options.address, host_address)?;
    // Admit the reverse flow through stateful host firewalls before `Start`.
    if let Err(error) = confirm_reverse_flow(&tx_sink, Duration::from_secs(5)) {
        return capture.finish_with(Err(error));
    }
    let fixture_capture = RxCapture::start(
        &context.lab.station_fixture,
        std::net::SocketAddrV4::new(options.address, options.port),
        output,
        options.duration,
        match options.phy {
            Phy::Ht40 => oer_hil_scenario_catalog::link::PhyExpectation::Ht40,
            Phy::He20 => oer_hil_scenario_catalog::link::PhyExpectation::He20,
        },
        fixture_guard_interval,
        None,
    )?;
    let fixture_tx_capture = match &context.lab.station_fixture {
        StationFixtureConfig::LocalLinux(config) => Some(LocalLinuxTxCapture::start(
            config,
            options.address,
            DEVICE_TX_SOURCE_PORT,
            options.tx_port,
            options.duration,
            match options.phy {
                Phy::Ht40 => oer_hil_scenario_catalog::link::PhyExpectation::Ht40,
                Phy::He20 => oer_hil_scenario_catalog::link::PhyExpectation::He20,
            },
        )?),
        StationFixtureConfig::OpenWrt(_) | StationFixtureConfig::External(_) => None,
    };
    let host_wire_capture = if context.lab.air_observer.is_some() {
        Some(host_route.capture_wire(options.address, output, options.duration)?)
    } else {
        None
    };
    let remote_air_capture =
        oer_hil_family_ieee80211_fixture::openwrt::air_monitor::Capture::start(
            context.lab,
            Some(options.address),
            options.duration,
            output,
        )?;
    let tx_monitor_capture = if capture_openwrt_tx_monitor_rx {
        let StationFixtureConfig::OpenWrt(config) = &context.lab.station_fixture else {
            return Err("OpenWrt TX-monitor evidence requires an OpenWrt station fixture".into());
        };
        Some(OpenWrtTxMonitorCapture::start(
            config,
            options.address,
            options.port,
            options.duration,
            output,
        )?)
    } else {
        None
    };
    let independent_air_capture = if capture_independent_laptop_air_monitor {
        let StationFixtureConfig::OpenWrt(config) = &context.lab.station_fixture else {
            return Err("independent laptop evidence requires an OpenWrt station fixture".into());
        };
        Some(LocalAirMonitorCapture::start(
            config,
            options.address,
            options.duration,
            output,
        )?)
    } else {
        None
    };
    let receiver = Receiver::start(
        &tx_sink,
        options.address,
        options.duration + Duration::from_secs(5) + oer_hil_net_traffic::SESSION_START_TIMEOUT,
        output,
        "station",
    )?;
    let session = capture.start_session(SessionConfig {
        network_interface: oer_hil_protocol::wifi::WifiNetworkInterface::Station,
        transport: Transport::Udp,
        direction: Direction::Bidirectional,
        completion: Completion::DurationMillis(u32::try_from(options.duration.as_millis())?),
        flows: [
            Some(SessionFlowConfig {
                flow_id: 0,
                peer: Some(Ipv4Endpoint {
                    address: host_address.octets(),
                    port: options.tx_port,
                }),
                target_rx: Some(FlowConfig {
                    payload_bytes: u16::try_from(options.payload)?,
                    offered_rate_bps: Some(options.rate_bps),
                    pacing_group_datagrams: None,
                }),
                target_tx: Some(FlowConfig {
                    payload_bytes: u16::try_from(options.tx_payload)?,
                    offered_rate_bps: options.tx_rate_bps,
                    pacing_group_datagrams: None,
                }),
                payload_identity: None,
            }),
            None,
        ],
        link_requirements: SessionLinkRequirements::tx_block_ack(0),
    })?;
    let host_result = send_paced_udp(
        host_route.source(),
        PacedUdpConfig {
            address: options.address,
            port: options.port,
            rate_bps: options.rate_bps,
            duration: options.duration,
            payload: options.payload,
        },
    );
    let structured = capture.wait_for_session(session, Duration::from_secs(5));
    let tx_bursts = receiver.finish(
        structured
            .as_ref()
            .ok()
            .map(|evidence| evidence.transport.tx_units),
    )?;
    let host = host_result?;
    context.results.replace(
        "delivery-progress",
        &progress::snapshot(host, &tx_bursts, None),
    );
    let structured = match structured {
        Ok(evidence) => evidence,
        Err(error) => return capture.finish_with(Err(error)),
    };
    if let Err(error) = capture.acknowledge_session(session) {
        return capture.finish_with(Err(error));
    }
    if let Some(wire) = host_wire_capture {
        wire.finish()?;
    }
    if let Some(observer) = remote_air_capture {
        observer.finish()?;
    }
    let fixture_rx = fixture_capture.map(RxCapture::finish).transpose()?;
    let fixture_tx = fixture_tx_capture
        .map(LocalLinuxTxCapture::finish)
        .transpose()?;
    let tx_monitor_rx = tx_monitor_capture
        .map(|capture| capture.finish(host.datagrams))
        .transpose()?;
    let independent_air_rx = independent_air_capture
        .map(LocalAirMonitorCapture::finish)
        .transpose()?;
    if let Some(evidence) = fixture_tx.as_ref() {
        context.results.observe("local-wireless-ingress", evidence);
    }
    let beacon_loss = require_no_beacon_loss.then(|| capture.require_no_beacon_loss());
    let log = capture.finish()?;
    context.results.replace(
        "delivery-progress",
        &progress::snapshot(host, &tx_bursts, Some(structured.transport)),
    );
    if let Some(result) = beacon_loss {
        result?;
    }
    super::continuity::require_rx_silence(options.maximum_rx_silence_ms, structured.transport)?;
    let rx_median = structured
        .transport
        .rx_bytes
        .saturating_mul(8)
        .saturating_mul(1_000)
        .checked_div(structured.transport.elapsed_micros.max(1))
        .unwrap_or(0);
    let tx_floor = structured
        .transport
        .tx_bytes
        .saturating_mul(8)
        .saturating_mul(1_000)
        .checked_div(structured.transport.elapsed_micros.max(1))
        .unwrap_or(0);
    let qualified_tx_bursts: Vec<_> = tx_bursts
        .iter()
        .copied()
        .filter(|burst| burst.started_at_zero && burst.datagrams >= MIN_HOST_TX_DATAGRAMS)
        .collect();
    if qualified_tx_bursts.len() != 1 {
        return Err(format!(
            "expected one complete target-to-host burst, received {}; {}",
            qualified_tx_bursts.len(),
            describe_bursts(&tx_bursts),
        )
        .into());
    }
    let host_tx = qualified_tx_bursts[0];
    if !require_driver_observation {
        if structured.radio.is_some()
            || structured.tx_timing.is_some()
            || structured.rx_delivery.is_some()
            || structured.network_scheduler.is_some()
        {
            return Err("performance image published driver-internal evidence".into());
        }
        let expected_rx_bytes = structured
            .transport
            .rx_units
            .saturating_mul(options.payload as u64);
        let expected_tx_bytes = structured
            .transport
            .tx_units
            .saturating_mul(options.tx_payload as u64);
        if !structured.finished.summary.verdict.passed()
            || structured.transport.transport_errors != 0
            || structured.transport.rx_bytes != expected_rx_bytes
            || structured.transport.tx_bytes != expected_tx_bytes
        {
            return Err(format!(
                "target did not complete bidirectional performance session cleanly: verdict={} errors={} rx={}/{} tx={}/{}",
                structured.finished.summary.verdict,
                structured.transport.transport_errors,
                structured.transport.rx_bytes,
                expected_rx_bytes,
                structured.transport.tx_bytes,
                expected_tx_bytes,
            )
            .into());
        }
        let minimum_rx_bps = options
            .rx_floor_bps
            .unwrap_or_else(|| options.rate_bps.saturating_mul(9) / 10);
        if host.throughput_bps() < minimum_rx_bps
            || rx_median.saturating_mul(1_000) < minimum_rx_bps
        {
            return Err(format!(
                "concurrent RX is below the configured floor: required={minimum_rx_bps} host={} target={} bit/s",
                host.throughput_bps(),
                rx_median.saturating_mul(1_000),
            )
            .into());
        }
        if let Some(required_tx_bps) = options.tx_floor_bps {
            let measured_device = tx_floor.saturating_mul(1_000);
            let measured_host = host_tx.throughput_kbps().saturating_mul(1_000);
            if measured_device < required_tx_bps || measured_host < required_tx_bps {
                return Err(format!(
                    "concurrent TX is below the configured floor: required={required_tx_bps} device={measured_device} host={measured_host} bit/s"
                )
                .into());
            }
        }
        if let Some(required_combined_bps) = options.combined_floor_bps {
            let measured = rx_median.saturating_add(tx_floor).saturating_mul(1_000);
            if measured < required_combined_bps {
                return Err(format!(
                    "combined throughput is below the configured floor: required={required_combined_bps} measured={measured} bit/s"
                )
                .into());
            }
        }
        context.results.observe(
            "bidirectional",
            &BidirectionalPerformanceObservation {
                config: &options,
                host_offer: host,
                host_sink: host_tx,
                session: structured,
                rx_kbps: rx_median,
                tx_kbps: tx_floor,
                host_receive_buffer_bytes,
            },
        );
        eprintln!(
            "OPENRADIOHOST result=PASS mode={}-bidirectional-performance offered_kbps={} host_kbps={} rx_kbps={rx_median} tx_kbps={tx_floor} host_tx_kbps={} combined_kbps={}",
            options.phy.name(),
            options.rate_bps / 1_000,
            host.throughput_bps() / 1_000,
            host_tx.throughput_kbps(),
            rx_median.saturating_add(tx_floor),
        );
        return Ok(());
    }
    let report = parse_device_report(&log);
    let raw_rx_radio = structured
        .radio
        .and_then(|evidence| evidence.rx)
        .ok_or("session did not publish typed RX radio evidence")?;
    let mut qualification_failure = if require_exact_delivery {
        structured.require_rx_radio(options.phy.expected_rx_format(), host.datagrams)
    } else {
        structured.require_rx_radio_health(options.phy.expected_rx_format())
    }
    .err()
    .map(|error| error.to_string());
    if options.phy == Phy::Ht40
        && let Err(error) = validate_ht40_rx_vector(&raw_rx_radio, minimum_mcs, guard_interval)
    {
        qualification_failure.get_or_insert_with(|| error.to_string());
    }
    let text_assessment = match assess_rx_report(&report, options.phy.expected_rx_format()) {
        Ok(assessment) => Some(assessment),
        Err(error) => {
            eprintln!("diagnostic_text_warning={error}");
            None
        }
    };
    if let Some(failure) = text_assessment
        .as_ref()
        .and_then(|assessment| assessment.failure.as_deref())
    {
        eprintln!("diagnostic_text_warning={failure}");
    }
    let typed_rx = RxQualification::from_typed(structured.transport, raw_rx_radio);
    let rx = text_assessment
        .map(|assessment| assessment.rx.with_typed_radio(&typed_rx))
        .unwrap_or_else(|| {
            let mut rx = typed_rx;
            // A failed typed acceptance result must not hide independent
            // diagnostic poll evidence from the same completed interval.
            rx.task_polls = report.task_polls;
            rx
        });
    context.measurements.record(rx.task_polls.measurements());
    if let Some(required_floor) = options.tx_floor_bps {
        let measured_device = tx_floor.saturating_mul(1_000);
        let measured_host = host_tx.throughput_kbps().saturating_mul(1_000);
        if measured_device < required_floor || measured_host < required_floor {
            qualification_failure.get_or_insert_with(|| format!(
                "concurrent TX is below the configured floor: required={} device={} host={} bit/s",
                required_floor, measured_device, measured_host
            ));
        }
    }
    if let Some(required_floor) = options.combined_floor_bps {
        let measured = rx_median.saturating_add(tx_floor).saturating_mul(1_000);
        if measured < required_floor {
            qualification_failure.get_or_insert_with(|| {
                format!(
                    "combined device throughput is below the configured floor: required={} measured={} bit/s",
                    required_floor, measured
                )
            });
        }
    }
    let minimum_bps = options
        .rx_floor_bps
        .unwrap_or_else(|| options.rate_bps.saturating_mul(9) / 10);
    if host.throughput_bps() < minimum_bps {
        qualification_failure.get_or_insert_with(|| {
            String::from("host failed to offer at least 90% of the requested rate")
        });
    } else if rx.throughput_median_kbps < minimum_bps / 1_000 {
        qualification_failure.get_or_insert_with(|| {
            format!(
                "device RX {} kbit/s is below the acceptance floor",
                rx.throughput_median_kbps,
            )
        });
    }
    if require_exact_delivery && let Some(fixture_rx) = fixture_rx.as_ref() {
        let expected_fixture_packets = host.datagrams.saturating_add(1);
        if fixture_rx.wireless_packets() != expected_fixture_packets {
            qualification_failure.get_or_insert_with(|| {
                format!(
                    "host/AP Wi-Fi egress mismatch: expected={} observed={} packets",
                    expected_fixture_packets,
                    fixture_rx.wireless_packets()
                )
            });
        }
    }
    let (required_width, minimum_rate) = options.phy.required_tx();
    let typed_tx = match structured.require_tx_radio(
        required_width,
        minimum_rate,
        u32::try_from(MIN_QUALIFIED_AGGREGATES).unwrap_or(u32::MAX),
    ) {
        Ok(evidence) => Some(evidence),
        Err(error) => {
            qualification_failure.get_or_insert_with(|| error.to_string());
            structured
                .radio
                .and_then(|radio| radio.tx)
                .zip(structured.tx_timing)
        }
    };
    {
        let evidence = structured;
        let expected_rx_bytes = evidence
            .transport
            .rx_units
            .saturating_mul(options.payload as u64);
        if !evidence.finished.summary.verdict.passed() || evidence.transport.transport_errors != 0 {
            qualification_failure.get_or_insert_with(|| {
                format!(
                    "target did not complete bidirectional session cleanly: verdict={} errors={}",
                    evidence.finished.summary.verdict, evidence.transport.transport_errors
                )
            });
        }
        if require_exact_delivery && let Some(delivery) = evidence.rx_delivery {
            let assessment = evidence::rx_delivery::assess(host.datagrams, delivery);
            if !assessment.exact() {
                qualification_failure.get_or_insert_with(|| {
                    format!(
                        "typed bidirectional RX delivery frontier is {}",
                        assessment.frontier()
                    )
                });
            }
        }
        if require_exact_delivery
            && (evidence.transport.rx_bytes != expected_rx_bytes
                || evidence.transport.rx_units != host.datagrams
                || evidence.transport.rx_bytes != host.bytes)
        {
            qualification_failure.get_or_insert_with(|| {
                format!(
                    "host/target bidirectional RX delivery mismatch: host={}/{} target={}/{}",
                    host.bytes,
                    host.datagrams,
                    evidence.transport.rx_bytes,
                    evidence.transport.rx_units
                )
            });
        }
    }
    let ampdu = typed_tx
        .map(|(tx, timing)| AmpduEvidence::from_typed(tx, timing))
        .unwrap_or_default();
    if require_exact_delivery
        && (host_tx.missing != 0 || host_tx.reordered != 0 || host_tx.duplicates != 0)
    {
        let target_tx_units = Some(structured.transport.tx_units);
        let post_block_ack_loss = target_tx_units
            .and_then(|units| post_block_ack_delivery_loss_lower_bound(ampdu, host_tx, units));
        qualification_failure.get_or_insert_with(|| format!(
            "host observed concurrent TX sequence defects: missing={} reordered={} duplicates={} \
             missing_runs={} maximum_missing_run={} maximum_missing_range={:?}..={:?} \
             target_tx_units={target_tx_units:?} ampdu_subframes={} block_acknowledged={} \
             post_block_ack_delivery_loss_lower_bound={post_block_ack_loss:?}",
            host_tx.missing,
            host_tx.reordered,
            host_tx.duplicates,
            host_tx.missing_runs,
            host_tx.maximum_missing_run,
            host_tx.maximum_missing_run_start,
            host_tx.maximum_missing_run_end,
            ampdu.subframes,
            ampdu.acknowledged,
        ));
    }
    if require_exact_delivery
        && (structured.transport.tx_units != host_tx.datagrams
            || structured.transport.tx_bytes != host_tx.bytes)
    {
        qualification_failure.get_or_insert_with(|| {
            format!(
                "typed/host bidirectional TX mismatch: target={}/{} host={}/{}",
                structured.transport.tx_bytes,
                structured.transport.tx_units,
                host_tx.bytes,
                host_tx.datagrams
            )
        });
    }
    if require_exact_delivery
        && let Some(fixture_tx) = fixture_tx.as_ref()
        && fixture_tx.udp_packets != structured.transport.tx_units
    {
        qualification_failure.get_or_insert_with(|| {
            format!(
                "target/local wireless bidirectional TX mismatch: target={} local_ingress={}",
                structured.transport.tx_units, fixture_tx.udp_packets
            )
        });
    }
    let observed = BidirectionalEvidence {
        host_offer: host,
        target_rx: rx,
        target_tx_floor_kbps: tx_floor,
        host_sink: host_tx,
        session: structured,
        ampdu,
        host_receive_buffer_bytes,
        fixture_rx,
        fixture_tx,
        tx_monitor_rx,
        independent_air_rx,
    };
    context.results.observe(
        "bidirectional",
        &BidirectionalObservation {
            config: &options,
            delivery: if require_exact_delivery {
                "exact"
            } else {
                "performance-health"
            },
            air_correlation: observed
                .tx_monitor_rx
                .as_ref()
                .zip(observed.independent_air_rx.as_ref())
                .map(|(ap, observer)| AirCorrelation::of(ap, observer)),
            evidence: &observed,
            rx_delivery: structured
                .rx_delivery
                .map(|delivery| evidence::rx_delivery::assess(host.datagrams, delivery)),
            failure: qualification_failure.as_deref(),
        },
    );
    if let Some(failure) = qualification_failure {
        return Err(failure.into());
    }
    eprintln!(
        "OPENRADIOHOST result=PASS mode={}-bidirectional offered_kbps={} \
         host_kbps={} rx_median_kbps={rx_median} concurrent_tx_floor_kbps={tx_floor} \
         host_tx_kbps={} \
         host_receive_buffer_bytes={} \
         ampdu_avg_subframes={:.2} ampdu_max_subframes={} full32={} \
         combined_floor_sum_kbps={}",
        options.phy.name(),
        options.rate_bps / 1_000,
        host.throughput_bps() / 1_000,
        host_tx.throughput_kbps(),
        host_receive_buffer_bytes,
        ampdu.subframes as f64 / ampdu.aggregates.max(1) as f64,
        ampdu.maximum,
        ampdu.full32,
        rx_median.saturating_add(tx_floor),
    );
    Ok(())
}

/// Minimum number of MPDUs positively acknowledged by the peer but absent
/// from the host's unique UDP delivery set.
///
/// The comparison is meaningful only when every typed target transmission was
/// represented by one A-MPDU subframe. A larger BlockAck set than host set
/// proves that at least the cardinality difference disappeared after the peer
/// accepted the MPDUs; it does not assume which individual sequences overlap.
pub fn post_block_ack_delivery_loss_lower_bound(
    ampdu: AmpduEvidence,
    host: Burst,
    target_tx_units: u64,
) -> Option<u64> {
    if ampdu.subframes != target_tx_units {
        return None;
    }
    let unique_host_datagrams = host.datagrams.saturating_sub(host.duplicates);
    Some(ampdu.acknowledged.saturating_sub(unique_host_datagrams))
}

impl Default for Config {
    fn default() -> Self {
        Self {
            maximum_rx_silence_ms: None,
            address: Ipv4Addr::UNSPECIFIED,
            port: DEFAULT_PORT,
            rate_bps: DEFAULT_RATE_BPS,
            rx_floor_bps: None,
            duration: DEFAULT_DURATION,
            payload: DEFAULT_PAYLOAD,
            tx_port: DEFAULT_TX_PORT,
            tx_payload: DEFAULT_TX_PAYLOAD,
            tx_rate_bps: None,
            tx_floor_bps: None,
            combined_floor_bps: None,
            phy: Phy::He20,
        }
    }
}
impl Config {
    fn validate(mut self) -> Result<Self> {
        if !(Duration::from_secs(5)..=Duration::from_secs(300)).contains(&self.duration) {
            return Err("traffic duration must be in 5..=300 seconds".into());
        }
        if !(64..=1472).contains(&self.payload) {
            return Err("UDP payload must be in 64..=1472 bytes".into());
        }
        if !(64..=1472).contains(&self.tx_payload) {
            return Err("UDP TX payload must be in 64..=1472 bytes".into());
        }
        if [
            Some(self.rate_bps),
            self.rx_floor_bps,
            self.tx_rate_bps,
            self.tx_floor_bps,
            self.combined_floor_bps,
        ]
        .into_iter()
        .flatten()
        .any(|rate| !(100_000..=500_000_000).contains(&rate))
        {
            return Err("traffic rate is outside the supported range".into());
        }
        if self.port == 0 || self.tx_port == 0 {
            return Err("RX and TX ports must be nonzero".into());
        }
        if self.rx_floor_bps.is_some_and(|floor| floor > self.rate_bps) {
            return Err("RX floor cannot exceed offered rate".into());
        }
        if self.tx_floor_bps.is_none() {
            self.tx_floor_bps = self.tx_rate_bps.map(|rate| rate.saturating_mul(9) / 10);
        }
        if self
            .tx_rate_bps
            .zip(self.tx_floor_bps)
            .is_some_and(|(rate, floor)| floor > rate)
        {
            return Err("TX floor cannot exceed TX offered rate".into());
        }
        if self.combined_floor_bps.is_some_and(|floor| {
            self.tx_rate_bps
                .and_then(|tx| self.rate_bps.checked_add(tx))
                .is_none_or(|offered| floor > offered)
        }) {
            return Err(
                "combined floor requires TX offered rate and cannot exceed the offered sum".into(),
            );
        }

        Ok(self)
    }
}

mod model;
mod parse;
mod qualify;
mod report;
#[cfg(test)]
mod tests;

use model::{
    AmpduBlockAckSample, AmpduHistogramSample, AmpduSample, AmpduTimingSample, DeviceReport,
    ThroughputSample, TxIrqTimingSample, TxSample,
};
pub use model::{
    AmpduEvidence, MacIrqEvidence, RxAmpduEvidence, RxAssessment, RxOrderEvidence,
    RxPipelineEvidence, RxQualification, RxReorderEvidence, RxSmpduEvidence, TaskPollSet,
    TxQualification, UdpSequenceEvidence, task_polls_from_log, validate_ht40_rx_vector,
};
use parse::{field, parse_device_report, text_field};
pub use qualify::assess_rx_log;
#[cfg(test)]
pub use qualify::validate_exact_rx_delivery;
use qualify::{assess_rx_report, is_qualified_rx_sample};
#[cfg(test)]
use qualify::{qualify_rx_report, qualify_tx_samples};
mod progress;

use report::{AirCorrelation, BidirectionalObservation, BidirectionalPerformanceObservation};
