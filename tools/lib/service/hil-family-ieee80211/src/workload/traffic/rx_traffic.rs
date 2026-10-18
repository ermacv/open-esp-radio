//! Host sender and typed result for production RX-only qualification.

use crate::link::WifiCapture as _;
use oer_hil_net_traffic::NetworkSession as _;
use oer_hil_workload::context::Context;
use std::{
    env, fs,
    net::Ipv4Addr,
    path::Path,
    time::{Duration, Instant},
};

use oer_hil_workload::{interrupt_latency, phy_tracking_hold as tracking_hold};

use oer_hil_protocol::{
    network::Completion, network::Direction, network::FlowConfig, network::SessionConfig,
    network::SessionFlowConfig, network::SessionLinkRequirements, network::Transport,
};

use crate::{
    Result, workload::traffic::bidirectional::RxQualification,
    workload::traffic::bidirectional::assess_rx_log,
    workload::traffic::bidirectional::validate_ht40_rx_vector,
};
use oer_hil_family_ieee80211_evidence as evidence;
use oer_hil_family_ieee80211_fixture::{
    host_network::BenchmarkIpv4Route, local::air_monitor::LocalAirMonitorCapture,
    local::air_monitor::LocalAirMonitorEvidence, openwrt::tx_monitor::OpenWrtTxMonitorCapture,
    openwrt::tx_monitor::OpenWrtTxMonitorEvidence, station_fixture::RxCapture,
    station_fixture::RxEvidence,
};
use oer_hil_lab::config::StationFixtureConfig;
use oer_hil_net_traffic::await_udp_rx_ready;
use oer_hil_net_traffic::{
    SessionEvidence, paced_udp::Config as PacedUdpConfig, paced_udp::HostTransmission,
    paced_udp::send as send_paced_udp,
};
use oer_hil_scenario_catalog::link::{HtGuardIntervalExpectation, PhyExpectation};
use serde::Serialize;

const DEFAULT_PORT: u16 = 4_323;
const DEFAULT_RATE_BPS: u64 = 20_000_000;
const DEFAULT_DURATION: Duration = Duration::from_secs(12);
const DEFAULT_PAYLOAD: usize = 1_200;
const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Eq, PartialEq)]
pub struct Config {
    pub maximum_rx_silence_ms: Option<u32>,
    pub address: Ipv4Addr,
    pub port: u16,
    pub rate_bps: u64,
    pub minimum_rate_bps: Option<u64>,
    pub duration: Duration,
    pub payload: usize,
    pub expected_rx_format: u8,
    pub phy: PhyExpectation,
    pub maximum_idle_channel_utilization_255: Option<u8>,
    /// Suspend the shared PHY's periodic tracking for the session.
    pub suspend_phy_tracking: bool,
    /// A device measurement taken during the traffic and again after it.
    pub traffic_probe: Option<TrafficProbe>,
}

/// A device measurement a receive session runs beside its traffic and again
/// on the idle link after it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrafficProbe {
    /// This many forced tracking ticks of each branch set
    /// (`oer_hil_workload::phy_tracking_hold`, ).
    TrackingHold(u16),
    /// This many software-interrupt latency samples
    /// (`oer_hil_workload::interrupt_latency`, ).
    InterruptLatency(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvidencePolicy {
    pub require_exact_delivery: bool,
    pub require_no_beacon_loss: bool,
    pub require_driver_observation: bool,
    pub capture_openwrt_tx_monitor: bool,
    pub capture_independent_laptop_monitor: bool,
    pub minimum_mcs: Option<u8>,
    pub guard_interval: HtGuardIntervalExpectation,
    pub fixture_guard_interval: HtGuardIntervalExpectation,
}

/// The typed result of one RX-only session: the host offer, the target's
/// session evidence, the fixture and air observers that ran, the qualified
/// driver counters when the image publishes them, and the acceptance failure.
#[derive(Serialize)]
struct RxObservation<'a> {
    phy: &'static str,
    delivery: &'static str,
    device: Ipv4Addr,
    requested_rate_bps: u64,
    host: HostTransmission,
    session: SessionEvidence,
    typed_rx_kbps: u64,
    rx: Option<&'a RxQualification>,
    fixture: Option<&'a RxEvidence>,
    openwrt_tx_monitor: Option<&'a OpenWrtTxMonitorEvidence>,
    independent_air: Option<&'a LocalAirMonitorEvidence>,
    /// Where delivery first diverged from the host's offer, when the image
    /// publishes the RX delivery frontier.
    rx_delivery: Option<evidence::rx_delivery::RxDeliveryAssessment>,
    failure: Option<&'a str>,
}

pub fn run(
    options: Config,
    output: &Path,
    context: &Context<'_>,
    evidence_policy: EvidencePolicy,
) -> Result<()> {
    let mut options = options.validate()?;
    let require_exact_delivery = evidence_policy.require_exact_delivery;
    fs::create_dir_all(output)?;
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
    let same_boot_probes = env::var("OPEN_RADIO_RX_SAME_BOOT_PROBES")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0);
    for probe in 1..=same_boot_probes {
        let probe_output = output.join(format!("same-boot-probe-{probe}"));
        std::fs::create_dir_all(&probe_output)?;
        let fixture_capture = RxCapture::start(
            &context.lab.station_fixture,
            std::net::SocketAddrV4::new(options.address, options.port),
            &probe_output,
            options.duration,
            options.phy,
            evidence_policy.fixture_guard_interval,
            options.maximum_idle_channel_utilization_255,
        )?;
        let duration_millis = u32::try_from(options.duration.as_millis())?;
        let session = capture.start_session(SessionConfig {
            network_interface: oer_hil_protocol::wifi::WifiNetworkInterface::Station,
            transport: Transport::Udp,
            direction: Direction::Rx,
            completion: Completion::DurationMillis(duration_millis),
            flows: [
                Some(SessionFlowConfig {
                    flow_id: 0,
                    peer: None,
                    target_rx: Some(FlowConfig {
                        payload_bytes: u16::try_from(options.payload)?,
                        offered_rate_bps: Some(options.rate_bps),
                        pacing_group_datagrams: None,
                    }),
                    target_tx: None,
                    payload_identity: None,
                }),
                None,
            ],
            link_requirements: SessionLinkRequirements::NONE,
        })?;
        let host = send_paced_udp(
            host_route.source(),
            PacedUdpConfig {
                address: options.address,
                port: options.port,
                rate_bps: options.rate_bps,
                duration: options.duration,
                payload: options.payload,
            },
        )?;
        host_route.verify_socket_source(host.source)?;
        let structured = capture.wait_for_session(
            session,
            options.duration.saturating_add(Duration::from_secs(10)),
        )?;
        capture.acknowledge_session(session)?;
        let fixture = fixture_capture.map(RxCapture::finish).transpose()?;
        let throughput_kbps = structured
            .transport
            .rx_bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(structured.transport.elapsed_micros.max(1))
            .unwrap_or(0);
        eprintln!(
            "OPENRADIOHOST same_boot_probe={probe}/{same_boot_probes} rx_kbps={throughput_kbps} rx_units={} elapsed_us={}",
            structured.transport.rx_units, structured.transport.elapsed_micros,
        );
        if let Some(fixture) = fixture {
            eprintln!(
                "OPENRADIOHOST same_boot_probe={probe}/{same_boot_probes} fixture={fixture:?}",
            );
        }
    }
    let fixture_capture = RxCapture::start(
        &context.lab.station_fixture,
        std::net::SocketAddrV4::new(options.address, options.port),
        output,
        options.duration,
        options.phy,
        evidence_policy.fixture_guard_interval,
        options.maximum_idle_channel_utilization_255,
    )?;
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
    let tx_monitor_capture = if evidence_policy.capture_openwrt_tx_monitor {
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
    let independent_air_capture = if evidence_policy.capture_independent_laptop_monitor {
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
    let duration_millis = u32::try_from(options.duration.as_millis())?;
    let tracking = match super::phy_tracking::begin(&capture, options.suspend_phy_tracking) {
        Ok(tracking) => tracking,
        Err(error) => return capture.finish_with(Err(error)),
    };
    // The timed ticks are the only tracking while the hold is measured.
    if let Some(TrafficProbe::TrackingHold(_)) = options.traffic_probe
        && let Err(error) = tracking_hold::suspend(&capture)
    {
        return capture.finish_with(Err(error));
    }
    let session = capture.start_session(SessionConfig {
        network_interface: oer_hil_protocol::wifi::WifiNetworkInterface::Station,
        transport: Transport::Udp,
        direction: Direction::Rx,
        completion: Completion::DurationMillis(duration_millis),
        flows: [
            Some(SessionFlowConfig {
                flow_id: 0,
                peer: None,
                target_rx: Some(FlowConfig {
                    payload_bytes: u16::try_from(options.payload)?,
                    offered_rate_bps: Some(options.rate_bps),
                    pacing_group_datagrams: None,
                }),
                target_tx: None,
                payload_identity: None,
            }),
            None,
        ],
        link_requirements: SessionLinkRequirements::NONE,
    })?;
    let send = || {
        send_paced_udp(
            host_route.source(),
            PacedUdpConfig {
                address: options.address,
                port: options.port,
                rate_bps: options.rate_bps,
                duration: options.duration,
                payload: options.payload,
            },
        )
    };
    // The probe's traffic measurement runs while the host's traffic does:
    // the traffic's interrupts compete with it on the device.
    let (host_result, mut traffic_measurement) = match options.traffic_probe {
        Some(probe) => std::thread::scope(|scope| {
            let deadline = Instant::now() + options.duration;
            let capture = &capture;
            let measured = scope.spawn(move || probe.measure_traffic(capture, deadline));
            let host = send();
            let measured = measured
                .join()
                .unwrap_or_else(|_| Err("the traffic probe thread panicked".into()));
            (host, Some(measured))
        }),
        None => (send(), None),
    };
    let host = host_result?;
    host_route.verify_socket_source(host.source)?;
    host_route.record(output, options.address, host.source)?;
    let structured = match capture.wait_for_session(
        session,
        options.duration.saturating_add(Duration::from_secs(10)),
    ) {
        Ok(evidence) => evidence,
        Err(error) => {
            return capture.finish_with(Err(error));
        }
    };
    if let Some(load) = &structured.cpu_load {
        context.measurements.cpu_load(load);
    }
    if let Err(error) = capture.acknowledge_session(session) {
        return capture.finish_with(Err(error));
    }
    if let (Some(probe), Some(measured)) = (options.traffic_probe, traffic_measurement.take())
        && let Err(error) = probe.finish_idle(&capture, &context.results, measured)
    {
        return capture.finish_with(Err(error));
    }
    if let Err(error) = super::phy_tracking::finish(&capture, &context.results, tracking) {
        return capture.finish_with(Err(error));
    }
    if let Some(wire) = host_wire_capture {
        wire.finish()?;
    }
    if let Some(observer) = remote_air_capture {
        observer.finish()?;
    }
    let fixture = fixture_capture.map(RxCapture::finish).transpose()?;
    let tx_monitor_rx = tx_monitor_capture
        .map(|capture| capture.finish(host.datagrams))
        .transpose()?;
    let independent_air_rx = independent_air_capture
        .map(LocalAirMonitorCapture::finish)
        .transpose()?;
    let beacon_loss = evidence_policy
        .require_no_beacon_loss
        .then(|| capture.require_no_beacon_loss());
    let log = capture.finish()?;
    context
        .measurements
        .record(super::bidirectional::task_polls_from_log(&log).measurements());
    if let Some(result) = beacon_loss {
        result?;
    }
    let minimum_bps = options
        .minimum_rate_bps
        .unwrap_or_else(|| options.rate_bps.saturating_mul(9) / 10);
    let typed_rx_kbps = structured
        .transport
        .rx_bytes
        .saturating_mul(8)
        .saturating_mul(1_000)
        .checked_div(structured.transport.elapsed_micros.max(1))
        .unwrap_or(0);
    record_rates(
        &context.measurements,
        typed_rx_kbps,
        host.throughput_bps(),
        minimum_bps,
    );
    super::continuity::record_rx_silence(
        &context.measurements,
        options.maximum_rx_silence_ms,
        structured.transport,
    );
    super::continuity::require_rx_silence(options.maximum_rx_silence_ms, structured.transport)?;
    if !evidence_policy.require_driver_observation {
        if structured.radio.is_some()
            || structured.tx_timing.is_some()
            || structured.rx_delivery.is_some()
            || structured.network_scheduler.is_some()
        {
            return Err("performance image published driver-internal evidence".into());
        }
        let expected_bytes = structured
            .transport
            .rx_units
            .saturating_mul(options.payload as u64);
        let link_failure = if options.phy == PhyExpectation::Ht40 {
            fixture.as_ref().map_or_else(
                || {
                    Some(String::from(
                        "HT40 performance requires a managed fixture link snapshot",
                    ))
                },
                |fixture| {
                    fixture
                        .require_ht40_downlink()
                        .err()
                        .map(|error| error.to_string())
                },
            )
        } else {
            None
        };
        let transport_failure = if !structured.finished.summary.verdict.passed() {
            Some(format!(
                "target did not complete the typed RX session normally: {}",
                structured.finished.summary.verdict,
            ))
        } else if structured.transport.tx_bytes != 0 || structured.transport.tx_units != 0 {
            Some(String::from(
                "RX-only session reported unexpected transmitted traffic",
            ))
        } else if structured.transport.transport_errors != 0 {
            Some(format!(
                "typed RX session reported {} transport errors",
                structured.transport.transport_errors
            ))
        } else if structured.transport.rx_bytes != expected_bytes {
            Some(format!(
                "typed RX byte count {} does not match {} full payload datagrams",
                structured.transport.rx_bytes, structured.transport.rx_units
            ))
        } else if host.throughput_bps() < minimum_bps {
            Some(String::from(
                "host failed to offer at least 90% of the requested RX rate",
            ))
        } else if typed_rx_kbps < minimum_bps / 1_000 {
            Some(format!(
                "device RX {typed_rx_kbps} kbit/s is below the acceptance floor"
            ))
        } else {
            None
        };
        let failure = link_failure.or(transport_failure);
        context.results.observe(
            "rx",
            &RxObservation {
                phy: options.phy.id(),
                delivery: "performance",
                device: options.address,
                requested_rate_bps: options.rate_bps,
                host,
                session: structured,
                typed_rx_kbps,
                rx: None,
                fixture: fixture.as_ref(),
                openwrt_tx_monitor: tx_monitor_rx.as_ref(),
                independent_air: independent_air_rx.as_ref(),
                rx_delivery: None,
                failure: failure.as_deref(),
            },
        );
        if let Some(failure) = failure {
            return Err(failure.into());
        }
        eprintln!(
            "OPENRADIOHOST result=PASS mode={}-rx-performance offered_kbps={} host_kbps={} rx_kbps={typed_rx_kbps}",
            options.phy.id(),
            options.rate_bps / 1_000,
            host.throughput_bps() / 1_000,
        );
        return Ok(());
    }
    let raw_rx_radio = structured
        .radio
        .and_then(|evidence| evidence.rx)
        .ok_or("session did not publish typed RX radio evidence")?;
    let typed_radio_failure = if require_exact_delivery {
        structured.require_rx_radio(options.expected_rx_format, host.datagrams)
    } else {
        structured.require_rx_radio_health(options.expected_rx_format)
    }
    .err()
    .map(|error| error.to_string());
    let typed_phy_failure = (options.phy == PhyExpectation::Ht40)
        .then(|| {
            validate_ht40_rx_vector(
                &raw_rx_radio,
                evidence_policy.minimum_mcs,
                evidence_policy.guard_interval,
            )
        })
        .transpose()
        .err()
        .map(|error| error.to_string());
    // Text telemetry enriches the report when present. Typed transport/radio
    // evidence alone decides qualification and therefore remains authoritative
    // even if the bounded diagnostic stream is truncated.
    let text_assessment = match assess_rx_log(&log, options.expected_rx_format) {
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
        .unwrap_or(typed_rx);
    let structured_failure = typed_phy_failure.or_else(|| {
        let evidence = structured;
        let expected_bytes = evidence
            .transport
            .rx_units
            .saturating_mul(options.payload as u64);
        if !evidence.finished.summary.verdict.passed() {
            Some(format!(
                "target did not complete the typed RX session normally: {}",
                evidence.finished.summary.verdict,
            ))
        } else if evidence.transport.tx_bytes != 0 || evidence.transport.tx_units != 0 {
            Some(String::from(
                "RX-only session reported unexpected transmitted traffic",
            ))
        } else if evidence.transport.transport_errors != 0 {
            Some(format!(
                "typed RX session reported {} transport errors",
                evidence.transport.transport_errors
            ))
        } else if evidence.transport.rx_bytes != expected_bytes {
            Some(format!(
                "typed RX byte count {} does not match {} full payload datagrams",
                evidence.transport.rx_bytes, evidence.transport.rx_units
            ))
        } else if require_exact_delivery
            && (evidence.transport.rx_delivered_units() != host.datagrams
                || evidence.transport.rx_delivered_bytes() != host.bytes)
        {
            Some(format!(
                "host/target RX delivery mismatch: host={}/{} target={}/{}",
                host.bytes,
                host.datagrams,
                evidence.transport.rx_delivered_bytes(),
                evidence.transport.rx_delivered_units()
            ))
        } else {
            None
        }
    });
    let typed_delivery_failure = if require_exact_delivery {
        structured.rx_delivery.and_then(|delivery| {
            let assessment = evidence::rx_delivery::assess(host.datagrams, delivery);
            (!assessment.exact()).then_some(format!(
                "typed RX delivery frontier is {}",
                assessment.frontier()
            ))
        })
    } else {
        None
    };
    let acceptance_failure = if host.throughput_bps() < minimum_bps {
        Some(String::from(
            "host failed to offer at least 90% of the requested RX rate",
        ))
    } else if typed_rx_kbps < minimum_bps / 1_000 {
        Some(format!(
            "device RX {} kbit/s is below the acceptance floor",
            typed_rx_kbps,
        ))
    } else {
        None
    };
    let fixture_failure = if require_exact_delivery {
        fixture.as_ref().and_then(|fixture| {
            let expected = host.datagrams.saturating_add(1);
            (fixture.wireless_packets() != expected).then_some({
                format!(
                    "host/AP Wi-Fi egress mismatch: expected={} observed={} packets",
                    expected,
                    fixture.wireless_packets()
                )
            })
        })
    } else {
        None
    };
    let failure = fixture_failure
        .or(typed_delivery_failure)
        .or(typed_radio_failure)
        .or(structured_failure)
        .or(acceptance_failure);
    context.results.observe(
        "rx",
        &RxObservation {
            phy: options.phy.id(),
            delivery: if require_exact_delivery {
                "exact"
            } else {
                "performance-health"
            },
            device: options.address,
            requested_rate_bps: options.rate_bps,
            host,
            session: structured,
            typed_rx_kbps,
            rx: Some(&rx),
            fixture: fixture.as_ref(),
            openwrt_tx_monitor: tx_monitor_rx.as_ref(),
            independent_air: independent_air_rx.as_ref(),
            rx_delivery: structured
                .rx_delivery
                .map(|delivery| evidence::rx_delivery::assess(host.datagrams, delivery)),
            failure: failure.as_deref(),
        },
    );
    if let Some(failure) = failure {
        return Err(failure.into());
    }
    eprintln!(
        "OPENRADIOHOST result=PASS mode={}-rx offered_kbps={} host_kbps={} \
         rx_median_kbps={} enqueued={} dropped=0",
        options.phy.id(),
        options.rate_bps / 1_000,
        host.throughput_bps() / 1_000,
        typed_rx_kbps,
        rx.enqueued,
    );
    Ok(())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            maximum_rx_silence_ms: None,
            address: Ipv4Addr::UNSPECIFIED,
            port: DEFAULT_PORT,
            rate_bps: DEFAULT_RATE_BPS,
            minimum_rate_bps: None,
            duration: DEFAULT_DURATION,
            payload: DEFAULT_PAYLOAD,
            expected_rx_format: 4,
            phy: PhyExpectation::He20,
            maximum_idle_channel_utilization_255: None,
            suspend_phy_tracking: false,
            traffic_probe: None,
        }
    }
}
impl Config {
    fn validate(self) -> Result<Self> {
        if !(Duration::from_secs(5)..=Duration::from_secs(300)).contains(&self.duration) {
            return Err("traffic duration must be in 5..=300 seconds".into());
        }
        if !(64..=1472).contains(&self.payload) {
            return Err("UDP payload must be in 64..=1472 bytes".into());
        }
        if self.maximum_idle_channel_utilization_255 == Some(0) {
            return Err("maximum idle channel utilization must be nonzero".into());
        }
        if [Some(self.rate_bps), self.minimum_rate_bps]
            .into_iter()
            .flatten()
            .any(|rate| !(100_000..=500_000_000).contains(&rate))
        {
            return Err("traffic rate is outside the supported range".into());
        }
        if self.port == 0 {
            return Err("port must be nonzero".into());
        }
        if self
            .minimum_rate_bps
            .is_some_and(|floor| floor > self.rate_bps)
        {
            return Err("throughput floor cannot exceed offered rate".into());
        }

        Ok(self)
    }
}

fn record_rates(
    recorder: &oer_hil_workload::measurements::Recorder,
    target_kbps: u64,
    host_bps: u64,
    minimum_bps: u64,
) {
    // The existing target gate compares integer kbit/s, while the host gate
    // compares bit/s. Preserve both resolutions in the reported thresholds.
    recorder.rate(
        "udp.rx.target-rate",
        target_kbps.saturating_mul(1_000),
        Some(minimum_bps / 1_000 * 1_000),
    );
    recorder.rate("udp.rx.host-offer-rate", host_bps, Some(minimum_bps));
}

#[cfg(test)]
mod tests;

/// The traffic half of a probe's measurement.
enum TrafficMeasurement {
    TrackingHold(Vec<tracking_hold::Sample>),
    InterruptLatency(interrupt_latency::Sample),
}

impl TrafficProbe {
    fn measure_traffic(
        self,
        capture: &oer_hil_link::SerialCapture,
        deadline: Instant,
    ) -> Result<TrafficMeasurement> {
        Ok(match self {
            Self::TrackingHold(ticks) => TrafficMeasurement::TrackingHold(tracking_hold::measure(
                capture,
                ticks,
                tracking_hold::Load::Traffic,
                deadline,
            )?),
            Self::InterruptLatency(samples) => {
                TrafficMeasurement::InterruptLatency(interrupt_latency::measure(
                    capture,
                    samples,
                    interrupt_latency::Load::Traffic,
                    Some(deadline.saturating_duration_since(Instant::now())),
                )?)
            }
        })
    }

    /// Measure again on the idle link, restore what the probe suspended and
    /// record both halves.
    fn finish_idle(
        self,
        capture: &oer_hil_link::SerialCapture,
        results: &oer_hil_workload::results::Results,
        traffic: Result<TrafficMeasurement>,
    ) -> Result<()> {
        match self {
            Self::TrackingHold(ticks) => {
                // The same ticks on the idle link, then the timer runs again.
                let idle = traffic.and_then(|traffic| {
                    let TrafficMeasurement::TrackingHold(mut samples) = traffic else {
                        unreachable!("a tracking hold measures ticks");
                    };
                    let deadline = Instant::now() + tracking_hold::window(ticks);
                    samples.extend(tracking_hold::measure(
                        capture,
                        ticks,
                        tracking_hold::Load::Idle,
                        deadline,
                    )?);
                    Ok(samples)
                });
                let resumed = tracking_hold::resume(capture);
                idle.and_then(|samples| tracking_hold::record(results, &samples))
                    .and(resumed)
            }
            Self::InterruptLatency(samples) => {
                let TrafficMeasurement::InterruptLatency(traffic) = traffic? else {
                    unreachable!("an interrupt-latency probe measures latencies");
                };
                let idle = interrupt_latency::measure(
                    capture,
                    samples,
                    interrupt_latency::Load::Idle,
                    None,
                )?;
                interrupt_latency::record(results, &[traffic, idle])
            }
        }
    }
}
