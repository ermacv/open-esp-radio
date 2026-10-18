//! Dispatch from a validated Wi-Fi scenario to its workload owner.

use std::{path::Path, time::Duration};

use oer_hil_scenario_catalog::link::PhyExpectation;
use oer_hil_schema::image::ImageClass;
use oer_hil_workload::context::Context;

use super::{
    AccessPoint, Direction, ProtectionPeer, RestartRf, RoleOperation, StationIcmp,
    StationReconnect, StationTcp, StationUdp, WifiScenario, WifiWorkload,
    station::PROTECTION_CHECKS,
};
use crate::{
    Result,
    workload::{
        ieee80211::{self, control::Operation},
        traffic,
    },
};
use oer_hil_family_ieee80211_evidence::air::MacAddress;
use oer_hil_family_ieee80211_fixture::{
    openwrt::air_monitor::ProtectionCapture, prepared::Prepared,
};
use oer_hil_protocol::wifi::WifiRadioRestartRf;

/// The UDP workloads claim A-MPDU and receive reorder evidence, which a
/// non-HT link does not have.
const LEGACY_UDP: &str = "UDP traffic workloads need an HT or HE link";

pub(super) fn execute(
    scenario: &WifiScenario,
    output: &Path,
    context: &Context<'_>,
    fixture: &Prepared,
) -> Result<()> {
    let image = scenario.image;
    let seconds = |value: u16| Duration::from_secs(u64::from(value));
    match &scenario.workload {
        WifiWorkload::StationUdp(workload) => station_udp(workload, image, output, context),
        WifiWorkload::StationTcp(workload) => station_tcp(workload, output, context),
        WifiWorkload::StationIcmp(workload) => station_icmp(workload, output, context),
        WifiWorkload::StationSleep(workload) => {
            ieee80211::station_sleep::run(workload, output, context, fixture)
        }
        WifiWorkload::StationReconnect(workload) => station_reconnect(workload, output, context),
        WifiWorkload::StationApLoss {
            link,
            timeout_seconds,
            require_recovery_echo,
        } => ieee80211::station_ap_loss::run(
            ieee80211::station_ap_loss::Config {
                require_recovery_echo: *require_recovery_echo,
                timeout: seconds(*timeout_seconds),
            },
            output,
            context,
            fixture,
            link.phy,
        ),
        WifiWorkload::StationApAbsence {
            link,
            timeout_seconds,
            initially_absent,
        } => ieee80211::station_ap_absence::run(
            ieee80211::station_ap_absence::Config {
                initially_absent: *initially_absent,
                timeout: seconds(*timeout_seconds),
            },
            output,
            context,
            fixture,
            link.phy,
        ),
        WifiWorkload::Role {
            link,
            timeout_seconds,
            operation,
        } => {
            let (operation, config) = role(*operation, seconds(*timeout_seconds));
            ieee80211::control::run(operation, config, output, context, *link)
        }
        WifiWorkload::MonitorCapture {
            link,
            timeout_seconds,
            duration_seconds,
            channel,
            snapshot_length,
        } => ieee80211::capture::run(
            ieee80211::capture::Config {
                timeout: seconds(*timeout_seconds),
                duration: Duration::from_secs(u64::from(*duration_seconds)),
                output: output.join("capture.pcapng"),
                channel: *channel,
                snapshot_length: *snapshot_length,
            },
            output,
            context,
            link.phy,
        ),
        WifiWorkload::AccessPoint(workload) => access_point(
            workload,
            ieee80211::access_point::Config {
                probe_load: workload.probe_load,
                cycles: workload.cycles,
                boots: workload.boots,
                timeout: seconds(workload.timeout_seconds),
                clients: workload.clients,
                security: workload.security,
                traffic: workload.traffic.clone(),
                link: workload.link,
                capture_independent_laptop_air_monitor: workload.independent_air_monitor,
                require_driver_observation: image.requires_driver_observation(),
                require_rx_delivery_evidence: matches!(image, ImageClass::DiagnosticRxDelivery),
            },
            output,
            context,
        ),
        WifiWorkload::StationAccessPoint(workload) => ieee80211::station_access_point::run(
            ieee80211::station_access_point::Config {
                timeout: seconds(workload.timeout_seconds),
                duration: seconds(workload.duration_seconds),
                direction: workload.direction,
                rate_bps_per_flow: workload.rate_bps_per_flow,
                minimum_bps_per_flow: workload.minimum_bps_per_flow,
                maximum_fairness_skew_percent: workload.maximum_fairness_skew_percent,
                payload_bytes: usize::from(workload.payload_bytes),
                require_driver_observation: image.requires_driver_observation(),
                capture_independent_laptop_air_monitor: workload.independent_air_monitor,
            },
            output,
            context,
        ),
        WifiWorkload::StationAccessPointReconnect {
            link,
            timeout_seconds,
        } => ieee80211::station_access_point_reconnect::run(
            seconds(*timeout_seconds),
            output,
            context,
            fixture,
            link.phy,
        ),
    }
}

/// Run station UDP, observing the target's protection when the workload
/// induces it. Both verdicts are reported; the traffic failure wins.
fn station_udp(
    workload: &StationUdp,
    image: ImageClass,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let Some(induced) = workload.induced_protection else {
        return station_udp_traffic(workload, image, output, context);
    };
    // The bound only stops a capture whose workload never finishes it.
    let bound = Duration::from_secs(u64::from(workload.duration_seconds) + 180);
    // The non-HT member is the laptop: its own data to the AP is not the
    // target's. An overlapping legacy BSS never joins the AP.
    let peer = match induced.peer {
        ProtectionPeer::NonHtMember => Some(laptop_address()?),
        ProtectionPeer::OverlappingLegacyBss => None,
    };
    let capture = ProtectionCapture::start(context.lab, true, bound, output)?;
    let traffic = station_udp_traffic(workload, image, output, context);
    let protection = assess_protection(
        capture,
        induced.minimum_protected_ppdu_percent,
        peer,
        context,
    );
    traffic.and(protection)
}

/// Run an AP workload, observing how the target protects its HT data to the
/// OpenWrt client while the laptop is a non-HT member.
fn access_point(
    workload: &AccessPoint,
    config: ieee80211::access_point::Config,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let Some(protection) = workload.protection else {
        return ieee80211::access_point::run(config, output, context);
    };
    let bound = config.timeout * u32::from(workload.boots) * u32::from(workload.cycles)
        + Duration::from_secs(180);
    let capture = ProtectionCapture::start(context.lab, false, bound, output)?;
    let traffic = ieee80211::access_point::run(config, output, context);
    // The capture stops even when the laptop's address is unknown.
    let peer = associated_laptop_address(output);
    let assessed = assess_protection(
        capture,
        protection.minimum_protected_ppdu_percent,
        peer.as_ref().ok().copied(),
        context,
    );
    let assessed = peer.and(assessed);
    traffic.and(assessed)
}

/// The address the laptop associated with the target AP: its interface
/// address before the workload may be a randomized one it never associates
/// with.
fn associated_laptop_address(output: &Path) -> Result<MacAddress> {
    match oer_hil_family_ieee80211_fixture::local::client::connected_station_addresses(output)?
        .as_slice()
    {
        [address] => Ok(address.parse()?),
        [] => Err("the laptop client recorded no associated station address".into()),
        addresses => Err(format!(
            "the laptop client associated with several station addresses: {addresses:?}"
        )
        .into()),
    }
}

/// The laptop radio's station address.
fn laptop_address() -> Result<MacAddress> {
    Ok(std::fs::read_to_string("/sys/class/net/wlan0/address")?
        .trim()
        .parse()?)
}

fn assess_protection(
    capture: ProtectionCapture,
    minimum_protected_ppdu_percent: u8,
    peer: Option<MacAddress>,
    context: &Context<'_>,
) -> Result<()> {
    use oer_hil_run_bundle_format::run::Comparison;
    use oer_hil_run_bundle_format::run::Measurement;
    use oer_hil_run_bundle_format::run::MeasurementUnit;
    let evidence = capture.finish(peer)?;
    if evidence.data_ppdus < MINIMUM_PROTECTION_PPDUS || evidence.nav_evaluated == 0 {
        return Err(format!(
            "protection observation is insufficient: {} data PPDUs, {} with an evaluable NAV",
            evidence.data_ppdus, evidence.nav_evaluated
        )
        .into());
    }
    let measurements = [
        Measurement::observed(
            PROTECTION_CHECKS[0],
            evidence.protected_basis_points(),
            MeasurementUnit::BasisPoints,
        )
        .evaluated(
            Comparison::AtLeast,
            u64::from(minimum_protected_ppdu_percent) * 100,
        ),
        Measurement::observed(
            PROTECTION_CHECKS[1],
            u64::from(evidence.wrong_control_rate),
            MeasurementUnit::Count,
        )
        .evaluated(Comparison::Exactly, 0),
        Measurement::observed(
            PROTECTION_CHECKS[2],
            u64::from(evidence.nav_short),
            MeasurementUnit::Count,
        )
        .evaluated(Comparison::Exactly, 0),
    ];
    let failed = measurements
        .iter()
        .filter(|measurement| {
            measurement.verdict == Some(oer_hil_run_bundle_format::run::MeasurementVerdict::Failed)
        })
        .map(|measurement| format!("{}={}", measurement.name, measurement.value))
        .collect::<Vec<_>>();
    context.measurements.record(measurements);
    if !failed.is_empty() {
        return Err(format!(
            "target did not follow the induced BSS protection: {}; {evidence:?}",
            failed.join(", ")
        )
        .into());
    }
    Ok(())
}

/// Fewer target PPDUs cannot establish a protection share.
const MINIMUM_PROTECTION_PPDUS: u32 = 50;

fn station_udp_traffic(
    workload: &StationUdp,
    image: ImageClass,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let duration = Duration::from_secs(u64::from(workload.duration_seconds));
    let payload = usize::from(workload.payload_bytes);
    let link = workload.link;
    let criteria = workload.criteria;
    let fixture_guard_interval = if workload.fixed_fixture_guard_interval {
        link.guard_interval
    } else {
        Default::default()
    };
    match workload.offer.direction() {
        Direction::Rx => traffic::rx_traffic::run(
            traffic::rx_traffic::Config {
                maximum_rx_silence_ms: criteria.maximum_rx_silence_ms,
                duration,
                payload,
                phy: link.phy,
                expected_rx_format: match link.phy {
                    PhyExpectation::Legacy => return Err(LEGACY_UDP.into()),
                    PhyExpectation::He20 => 4,
                    PhyExpectation::Ht20 | PhyExpectation::Ht40 => 2,
                },
                rate_bps: workload.offer.rx_bps.expect("validated RX offer"),
                minimum_rate_bps: criteria.minimum_rx_bps,
                maximum_idle_channel_utilization_255: criteria.maximum_idle_channel_utilization_255,
                suspend_phy_tracking: workload.suspend_phy_tracking,
                traffic_probe: workload
                    .phy_tracking_hold
                    .map(crate::workload::traffic::rx_traffic::TrafficProbe::TrackingHold)
                    .or(workload
                        .interrupt_latency_probe
                        .map(crate::workload::traffic::rx_traffic::TrafficProbe::InterruptLatency)),
                ..Default::default()
            },
            output,
            context,
            traffic::rx_traffic::EvidencePolicy {
                require_exact_delivery: criteria.exact_delivery,
                require_no_beacon_loss: criteria.require_no_beacon_loss,
                require_driver_observation: image.requires_driver_observation(),
                capture_openwrt_tx_monitor: workload.observation.openwrt_tx_monitor,
                capture_independent_laptop_monitor: workload.observation.independent_air_monitor,
                minimum_mcs: link.minimum_mcs,
                guard_interval: link.guard_interval,
                fixture_guard_interval,
            },
        ),
        Direction::Tx => {
            let (bandwidth_mhz, minimum_rate_kbps) = match link.phy {
                PhyExpectation::Legacy => return Err(LEGACY_UDP.into()),
                PhyExpectation::He20 => (20, 114_700),
                PhyExpectation::Ht40 | PhyExpectation::Ht20 => (40, 135_000),
            };
            traffic::tx_traffic::run(
                traffic::tx_traffic::Config {
                    duration,
                    payload,
                    bandwidth_mhz,
                    minimum_rate_kbps,
                    offered_rate_bps: workload.offer.tx_bps,
                    throughput_floor_bps: criteria.minimum_tx_bps,
                    maximum_idle_channel_utilization_255: criteria
                        .maximum_idle_channel_utilization_255,
                    suspend_phy_tracking: workload.suspend_phy_tracking,
                    ..Default::default()
                },
                output,
                context,
                criteria.exact_delivery,
                criteria.require_no_beacon_loss,
                image.requires_driver_observation(),
            )
        }
        Direction::Bidirectional => traffic::bidirectional::run(
            traffic::bidirectional::Config {
                maximum_rx_silence_ms: criteria.maximum_rx_silence_ms,
                duration,
                payload,
                phy: if link.phy == PhyExpectation::He20 {
                    traffic::bidirectional::Phy::He20
                } else {
                    traffic::bidirectional::Phy::Ht40
                },
                rate_bps: workload.offer.rx_bps.expect("validated RX offer"),
                tx_rate_bps: workload.offer.tx_bps,
                rx_floor_bps: criteria.minimum_rx_bps,
                tx_floor_bps: criteria.minimum_tx_bps,
                combined_floor_bps: criteria.minimum_combined_bps,
                ..Default::default()
            },
            output,
            context,
            traffic::bidirectional::RunPolicy {
                require_exact_delivery: criteria.exact_delivery,
                require_no_beacon_loss: criteria.require_no_beacon_loss,
                capture_openwrt_tx_monitor_rx: workload.observation.openwrt_tx_monitor,
                capture_independent_laptop_air_monitor: workload
                    .observation
                    .independent_air_monitor,
                require_driver_observation: image.requires_driver_observation(),
                minimum_mcs: link.minimum_mcs,
                guard_interval: link.guard_interval,
                fixture_guard_interval,
            },
        ),
    }
}

fn station_tcp(workload: &StationTcp, output: &Path, context: &Context<'_>) -> Result<()> {
    let direction = match workload.offer.direction() {
        Direction::Rx => oer_hil_protocol::network::Direction::Rx,
        Direction::Tx => oer_hil_protocol::network::Direction::Tx,
        Direction::Bidirectional => oer_hil_protocol::network::Direction::Bidirectional,
    };
    let defaults = traffic::tcp_traffic::Config::for_direction(direction);
    traffic::tcp_traffic::run(
        traffic::tcp_traffic::Config {
            duration: Duration::from_secs(u64::from(workload.duration_seconds)),
            chunk_bytes: usize::from(workload.chunk_bytes),
            rx_rate_bps: workload.offer.rx_bps,
            tx_rate_bps: workload.offer.tx_bps,
            rx_floor_bps: workload.criteria.minimum_rx_bps.or(defaults.rx_floor_bps),
            tx_floor_bps: workload.criteria.minimum_tx_bps.or(defaults.tx_floor_bps),
            ..defaults
        },
        output,
        context,
        workload.criteria.require_no_beacon_loss,
    )
}

fn station_icmp(workload: &StationIcmp, output: &Path, context: &Context<'_>) -> Result<()> {
    traffic::icmp_latency::run(
        traffic::icmp_latency::Config {
            count: workload.count,
            interval: Duration::from_millis(u64::from(workload.interval_ms)),
            timeout: Duration::from_millis(u64::from(workload.timeout_ms)),
            payload_bytes: usize::from(workload.payload_bytes),
            maximum_lost: workload.criteria.maximum_lost,
            maximum_p95: workload
                .criteria
                .maximum_p95_ms
                .map(|ms| Duration::from_millis(u64::from(ms))),
            ..Default::default()
        },
        output,
        context,
        workload.criteria.require_no_beacon_loss,
    )
}

fn station_reconnect(
    workload: &StationReconnect,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    ieee80211::station_lifecycle::run(
        ieee80211::station_lifecycle::Config {
            cycles: workload.cycles,
            boots: workload.boots,
            timeout: Duration::from_secs(u64::from(workload.timeout_seconds)),
            ..Default::default()
        },
        output,
        context,
        workload.criteria.require_no_beacon_loss,
    )
}

fn role(operation: RoleOperation, timeout: Duration) -> (Operation, ieee80211::control::Config) {
    let mut config = ieee80211::control::Config {
        timeout,
        restart_cycles: 1,
        restart_rf: WifiRadioRestartRf::ClosedAndWoken,
        monitor_channel: None,
        monitor_duration: Duration::from_secs(3),
        snapshot_length: 256,
    };
    let operation = match operation {
        RoleOperation::Stop {} => Operation::Stop,
        RoleOperation::Start {} => Operation::Start,
        RoleOperation::Restart { cycles, rf } => {
            config.restart_cycles = cycles;
            config.restart_rf = match rf {
                RestartRf::ClosedAndWoken => WifiRadioRestartRf::ClosedAndWoken,
                RestartRf::KeptOpen => WifiRadioRestartRf::KeptOpen,
            };
            Operation::Restart
        }
        RoleOperation::Scan {} => Operation::Scan,
        RoleOperation::Monitor {
            channel,
            dwell_seconds,
            snapshot_length,
        }
        | RoleOperation::Roundtrip {
            channel,
            dwell_seconds,
            snapshot_length,
        } => {
            config.monitor_channel = channel;
            config.monitor_duration = Duration::from_secs(u64::from(dwell_seconds));
            config.snapshot_length = snapshot_length;
            if matches!(operation, RoleOperation::Monitor { .. }) {
                Operation::Monitor
            } else {
                Operation::Roundtrip
            }
        }
        RoleOperation::AccessPoint {} => Operation::AccessPoint,
        RoleOperation::StationAccessPoint { dwell_seconds } => {
            config.monitor_duration = Duration::from_secs(u64::from(dwell_seconds));
            Operation::StationAccessPoint
        }
    };
    (operation, config)
}
