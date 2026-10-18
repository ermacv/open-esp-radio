//! The `[coexistence]` scenario table: Wi-Fi and Bluetooth LE traffic at
//! once on the joint image's shared radio.
//!
//! The target runs its Wi-Fi station and the Bluetooth LE GATT application
//! on one radio. The workload connects the Linux adapter to the GATT
//! application, then offers UDP to the station while an ATT echo load runs
//! over the Bluetooth connection for the same interval. Both radios must keep
//! working: the station must deliver at least the minimum UDP rate and the
//! echo load must complete its exchanges without a disconnect.

#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

use std::{
    net::Ipv4Addr,
    path::Path,
    time::{Duration, Instant},
};

use oer_hil_workload::phy_tracking_hold::{self as tracking_hold, Load};

use oer_hil_family_bluetooth::workload::bluetooth::coexistence::Echo;
use oer_hil_family_ieee80211_fixture::host_network::BenchmarkIpv4Route;
use oer_hil_lab::config::LabConfig;
use oer_hil_net_traffic::{
    NetworkSession as _, await_udp_rx_ready,
    paced_udp::{Config as PacedUdpConfig, send as send_paced_udp},
};
use oer_hil_protocol::{
    network::Completion, network::Direction, network::FlowConfig, network::SessionConfig,
    network::SessionFlowConfig, network::SessionLinkRequirements, network::Transport,
};
use oer_hil_run_bundle_format::run::Comparison;
use oer_hil_run_bundle_format::run::Measurement;
use oer_hil_run_bundle_format::run::MeasurementUnit;
use oer_hil_run_bundle_format::run::MeasurementVerdict;
use oer_hil_scenario::{AirUse, Plan, ScenarioFamily};
use oer_hil_scenario_catalog::link::{PhyExpectation, WifiLabUse};
use oer_hil_scenario_catalog::{bounded, requirements::Requirements};
use oer_hil_schema::image::ImageClass;
use oer_hil_workload::{
    Result,
    context::Context,
    family::{Kind, Workload},
    fixture::Fixtures,
};
use serde::{Deserialize, Serialize};

/// The coexistence family's key in a scenario document.
pub const FAMILY: Kind = Kind::of::<CoexistenceScenario>("coexistence");

const UDP_PORT: u16 = 4_323;
const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(45);
const CHECKS: [&str; 2] = ["coexistence.wifi.rx-rate", "coexistence.bluetooth.echoes"];

/// Concurrent station UDP receive and Bluetooth LE ATT echo traffic.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoexistenceScenario {
    /// The station fixture link.
    pub phy: PhyExpectation,
    /// The shared traffic interval.
    pub duration_seconds: u8,
    /// UDP rate the host offers the station.
    pub rx_bps: u64,
    pub payload_bytes: u16,
    /// The station's minimum delivered UDP rate while Bluetooth runs.
    pub minimum_rx_bps: u64,
    /// The minimum completed ATT write/read echoes in the interval.
    pub minimum_echoes: u32,
    /// Time this many forced shared-PHY tracking ticks of each branch set
    /// while both radios run, and again once their traffic ended ():
    /// the RF hold with both tracking classes as clients. It selects the
    /// `diagnostic-wifi-ble-coex-tracking-hold` image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phy_tracking_hold: Option<u16>,
}

impl ScenarioFamily for CoexistenceScenario {
    fn validate(&self) -> Result<()> {
        bounded(self.duration_seconds, 4, 16, "duration_seconds")?;
        bounded(self.payload_bytes, 64, 1_472, "payload_bytes")?;
        if self.minimum_rx_bps == 0 || self.minimum_rx_bps > self.rx_bps {
            return Err("minimum_rx_bps must be positive and at most rx_bps".into());
        }
        if self.minimum_echoes == 0 {
            return Err("minimum_echoes must be positive".into());
        }
        if let Some(ticks) = self.phy_tracking_hold {
            bounded(ticks, 1, 10, "phy_tracking_hold")?;
            let window = tracking_hold::window(ticks);
            if u64::from(self.duration_seconds) < window.as_secs() {
                return Err(format!(
                    "phy_tracking_hold of {ticks} ticks needs at least {} s of traffic",
                    window.as_secs()
                )
                .into());
            }
        }
        Ok(())
    }

    fn plan(&self) -> Plan {
        Plan {
            requirements: Requirements {
                station_network: true,
                bluetooth_adapter: true,
                ..Requirements::default()
            },
            checks: CHECKS.to_vec(),
            wifi: WifiLabUse {
                link: Some(self.phy),
                ..WifiLabUse::default()
            },
            ..Plan::target_only(if self.phy_tracking_hold.is_some() {
                ImageClass::DiagnosticWifiBleCoexTrackingHold
            } else {
                ImageClass::WifiBleCoex
            })
        }
    }

    /// Both radios share the 2.4 GHz band: the station's channel and the
    /// connection's hops.
    fn air_use(&self) -> Vec<AirUse> {
        vec![AirUse::Band2G4]
    }
}

impl Workload for CoexistenceScenario {
    fn precondition(&self, lab: &LabConfig) -> Result<()> {
        let adapter = lab
            .bluetooth_adapter
            .ok_or("missing [bluetooth] adapter in the stand file")?;
        oer_hil_family_bluetooth::fixture::bluetooth::att::preflight(adapter)
    }

    fn run(&self, output: &Path, context: &Context<'_>, _fixtures: &Fixtures) -> Result<()> {
        let adapter = context
            .lab
            .bluetooth_adapter
            .ok_or("set [bluetooth] adapter in the stand file")?;
        let duration = Duration::from_secs(u64::from(self.duration_seconds));
        context.with_capture(output, |capture| {
            let ready = await_udp_rx_ready(
                capture,
                context.target(),
                Ipv4Addr::UNSPECIFIED,
                UDP_PORT,
                DEVICE_READY_TIMEOUT,
            )?;
            let route = BenchmarkIpv4Route::discover(ready.address, &context.lab.station_fixture)?;
            let mut echo = Echo::connect(capture, adapter, output, &context.results)?;
            // The timed ticks are the only tracking while the hold is
            // measured.
            if self.phy_tracking_hold.is_some() {
                tracking_hold::suspend(capture)?;
            }
            let session = capture.start_session(SessionConfig {
                network_interface: oer_hil_protocol::wifi::WifiNetworkInterface::Station,
                transport: Transport::Udp,
                direction: Direction::Rx,
                completion: Completion::DurationMillis(u32::try_from(duration.as_millis())?),
                flows: [
                    Some(SessionFlowConfig {
                        flow_id: 0,
                        peer: None,
                        target_rx: Some(FlowConfig {
                            payload_bytes: self.payload_bytes,
                            offered_rate_bps: Some(self.rx_bps),
                            pacing_group_datagrams: None,
                        }),
                        target_tx: None,
                        payload_identity: None,
                    }),
                    None,
                ],
                link_requirements: SessionLinkRequirements::NONE,
            })?;
            // The echo load runs on its own thread for the interval the host
            // offers UDP; the capture serves it only after the interval. A
            // tracking hold is timed on a third thread over the same
            // interval.
            let (host, echoes, traffic_hold) = std::thread::scope(|scope| {
                let echoes = scope.spawn(|| echo.run(duration));
                let deadline = Instant::now() + duration;
                let hold = self.phy_tracking_hold.map(|ticks| {
                    scope.spawn(move || {
                        tracking_hold::measure(capture, ticks, Load::Traffic, deadline)
                    })
                });
                let host = send_paced_udp(
                    route.source(),
                    PacedUdpConfig {
                        address: ready.address,
                        port: UDP_PORT,
                        rate_bps: self.rx_bps,
                        duration,
                        payload: usize::from(self.payload_bytes),
                    },
                );
                let hold = hold.map(|hold| {
                    hold.join()
                        .unwrap_or_else(|_| Err("the tracking hold thread panicked".into()))
                });
                (host, echoes.join(), hold)
            });
            let echoes = echoes.map_err(|_| "echo load panicked")??;
            route.verify_socket_source(host?.source)?;
            let structured =
                capture.wait_for_session(session, duration + Duration::from_secs(10))?;
            capture.acknowledge_session(session)?;
            echo.finish()?;
            if let (Some(ticks), Some(samples)) = (self.phy_tracking_hold, traffic_hold) {
                // The same ticks once both radios' traffic ended, then the
                // timer runs again.
                let idle = samples.and_then(|mut samples| {
                    let deadline = Instant::now() + tracking_hold::window(ticks);
                    samples.extend(tracking_hold::measure(
                        capture,
                        ticks,
                        Load::Idle,
                        deadline,
                    )?);
                    Ok(samples)
                });
                let resumed = tracking_hold::resume(capture);
                idle.and_then(|samples| tracking_hold::record(&context.results, &samples))
                    .and(resumed)?;
            }
            let rx_bps = structured
                .transport
                .rx_bytes
                .saturating_mul(8)
                .saturating_mul(1_000_000)
                .checked_div(structured.transport.elapsed_micros.max(1))
                .unwrap_or(0);
            evaluate(self, context, rx_bps, echoes.echoes)
        })
    }
}

fn evaluate(
    scenario: &CoexistenceScenario,
    context: &Context<'_>,
    rx_bps: u64,
    echoes: u32,
) -> Result<()> {
    let measurements = vec![
        Measurement::observed(CHECKS[0], rx_bps, MeasurementUnit::BitsPerSecond)
            .evaluated(Comparison::AtLeast, scenario.minimum_rx_bps),
        Measurement::observed(CHECKS[1], u64::from(echoes), MeasurementUnit::Count)
            .evaluated(Comparison::AtLeast, u64::from(scenario.minimum_echoes)),
    ];
    let failed = measurements
        .iter()
        .filter(|measurement| measurement.verdict == Some(MeasurementVerdict::Failed))
        .map(|measurement| format!("{}={}", measurement.name, measurement.value))
        .collect::<Vec<_>>();
    context.measurements.record(measurements);
    eprintln!("coexistence rx_bps={rx_bps} echoes={echoes}");
    if !failed.is_empty() {
        return Err(format!("shared-radio traffic fell short: {}", failed.join(", ")).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
