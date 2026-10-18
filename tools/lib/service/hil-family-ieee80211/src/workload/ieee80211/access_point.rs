//! WPA2 AP lifecycle, exact data-plane and concurrent-client qualification.

use crate::link::WifiCapture as _;
use oer_hil_workload::context::Context;
use std::{fs, path::Path, time::Duration};

use oer_hil_protocol::{
    network::Direction as ProtocolDirection, wifi::WifiAccessPointSecurity, wifi::WifiRole,
};

use crate::scenario::{
    AccessPointClients, AccessPointTraffic, Direction, LinkExpectation, RateFloors,
};
use crate::{
    Result, workload::ieee80211::control::report_stack,
    workload::ieee80211::control::require_transition, workload::ieee80211::control::start_station,
    workload::ieee80211::control::stop_station,
};
use oer_hil_family_ieee80211_fixture::{
    local::air_monitor::LocalAirMonitorCapture, openwrt::client::ControlledOpenWrtClient,
    openwrt::client::OpenWrtClientLinkObservation,
};
use oer_hil_lab::config::StationFixtureConfig;
use oer_hil_link::SerialCapture;
use oer_hil_net_traffic::SessionEvidence;
use oer_hil_scenario_catalog::link::{HtGuardIntervalExpectation, PhyExpectation};

mod clients;
mod icmp;
mod multi_client;
mod progress;
mod report;
mod tcp;
mod udp;

use clients::{ConnectedClients, connect_clients, restore_clients};
use icmp::qualify_icmp;
use multi_client::qualify_multi_client_udp;
#[cfg(test)]
use multi_client::validate_multi_client_fairness;
#[cfg(test)]
use oer_hil_net_traffic::paced_tcp::{
    HostReception as TcpReception, HostTransmission as TcpTransmission,
};
#[cfg(test)]
use oer_hil_net_traffic::paced_udp::HostTransmission as UdpTransmission;
#[cfg(test)]
use oer_hil_protocol::network::Ipv4Endpoint;
#[cfg(test)]
use oer_hil_run_bundle_format::run::fixtures::Burst;
#[cfg(test)]
use report::MultiClientFlowReport;
use report::{
    ACCESS_POINT_REPORT_SCHEMA, AccessPointReport, BootReport, CycleReport, SessionReport,
    TrafficReport,
};
#[cfg(test)]
use std::net::Ipv4Addr;
#[cfg(test)]
use tcp::validate_tcp;
use tcp::{TcpWorkload, qualify_tcp};
#[cfg(test)]
use udp::{UdpEvidencePolicy, validate_udp};
use udp::{UdpWorkload, qualify_udp};

const UDP_RX_PORT: u16 = 4_323;
const UDP_TX_SOURCE_PORT: u16 = 4_324;
const UDP_HOST_PORT: u16 = 9_002;
const UDP_SECONDARY_HOST_PORT: u16 = 9_003;
const TCP_PORT: u16 = 4_325;

#[derive(Clone)]
pub struct Config {
    pub probe_load: bool,
    pub cycles: u8,
    pub boots: u8,
    pub timeout: Duration,
    pub clients: AccessPointClients,
    pub security: WifiAccessPointSecurity,
    pub traffic: AccessPointTraffic,
    pub link: Option<LinkExpectation>,
    pub require_driver_observation: bool,
    pub require_rx_delivery_evidence: bool,
    pub capture_independent_laptop_air_monitor: bool,
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    if let Some(expected_phy) = config.link.map(|link| link.phy) {
        let expected_bandwidth_mhz = match expected_phy {
            PhyExpectation::Ht40 => 40,
            PhyExpectation::Ht20 => 20,
            PhyExpectation::He20 => {
                return Err("AP qualification does not claim an HE20 PHY".into());
            }
            PhyExpectation::Legacy => {
                return Err("AP qualification does not claim a legacy PHY".into());
            }
        };
        if context.lab.access_point.bandwidth_mhz() != expected_bandwidth_mhz {
            return Err(format!(
                "AP scenario requires {} ({} MHz), but the context AP request is {} MHz",
                expected_phy.id(),
                expected_bandwidth_mhz,
                context.lab.access_point.bandwidth_mhz(),
            )
            .into());
        }
    }
    fs::create_dir_all(output)?;
    let fixture_preparation = if config.clients.openwrt() {
        let StationFixtureConfig::OpenWrt(fixture) = &context.lab.station_fixture else {
            return Err("AP controlled OpenWrt client requires the OpenWrt station fixture".into());
        };
        Some(ControlledOpenWrtClient::prepare_fixture(
            &context.lab.access_point,
            fixture,
        )?)
    } else {
        None
    };
    let mut report = AccessPointReport {
        schema: ACCESS_POINT_REPORT_SCHEMA,
        fixture_preparation,
        boots: Vec::with_capacity(usize::from(config.boots)),
    };
    for boot in 0..config.boots {
        let boot_output = if config.boots == 1 {
            output.to_owned()
        } else {
            output.join(format!("boot-{boot:02}"))
        };
        fs::create_dir_all(&boot_output)?;
        let mut cycles = Vec::with_capacity(usize::from(config.cycles));
        let result = context.with_capture(&boot_output, |capture| {
            qualify(capture, &config, context, &boot_output, &mut cycles)
        });
        // A task-poll image reports each traffic interval's poll residence
        // on the console; the last boot's reaches the measurements.
        if let Ok(log) = fs::read_to_string(boot_output.join("uart.log")) {
            context.measurements.record(
                crate::workload::traffic::bidirectional::task_polls_from_log(&log).measurements(),
            );
        }
        report.boots.push(BootReport {
            boot,
            cycles,
            error: result.as_ref().err().map(|error| error.to_string()),
        });
        context.results.replace("access-point", &report);
        result?;
    }
    Ok(())
}

fn qualify(
    capture: &SerialCapture,
    config: &Config,
    context: &Context<'_>,
    output: &Path,
    cycles: &mut Vec<CycleReport>,
) -> Result<()> {
    let capabilities = capture.prepare_station(context.target(), config.timeout)?;
    if !capabilities.has::<oer_hil_protocol::wifi::RoleControl>()
        || !capabilities.has::<oer_hil_protocol::wifi::AccessPoint>()
    {
        return Err("firmware does not advertise AP role control".into());
    }
    report_stack(capture, config.timeout, "ap-initial-station-connected")?;
    let minimum_clients = config.clients.count();
    if context.lab.access_point.client_limit() < minimum_clients {
        return Err(format!(
            "AP client_limit={} is below the scenario's {minimum_clients} concurrent clients",
            context.lab.access_point.client_limit(),
        )
        .into());
    }

    for cycle in 0..config.cycles {
        // Validate all host-owned inputs before releasing the connected STA.
        // An invalid AP request must not strand the target in Idle.
        let request = context.lab.access_point.protocol_request(config.security)?;
        let _ = stop_station(capture, config.timeout)?;
        if let Err(error) = report_stack(capture, config.timeout, "ap-station-stopped") {
            let restart = restore_connected_station(capture, config.timeout, context).err();
            return Err(with_cleanup_errors(error, None, None, None, restart));
        }
        let start = match capture.command(
            oer_hil_protocol::wifi::StartAccessPoint(request),
            oer_hil_link::PROTOCOL_READY_TIMEOUT,
        ) {
            Ok(start) => start,
            Err(error) => {
                let restart = restore_connected_station(capture, config.timeout, context).err();
                return Err(with_cleanup_errors(error, None, None, None, restart));
            }
        };
        let started = capture
            .wait_command::<oer_hil_protocol::wifi::AccessPointStarted>(start, config.timeout)
            .map(|completed| completed.0);
        let started = match started {
            Ok(started) => started,
            Err(error) => {
                let restart = restore_connected_station(capture, config.timeout, context).err();
                return Err(with_cleanup_errors(error, None, None, None, restart));
            }
        };
        if let Err(error) = require_transition(started, WifiRole::Idle, WifiRole::AccessPoint) {
            return Err(cleanup_after_client_failure(
                capture,
                config.timeout,
                started.generation,
                context,
                error,
            ));
        }
        if let Err(error) = report_stack(capture, config.timeout, "ap-started") {
            return Err(cleanup_after_client_failure(
                capture,
                config.timeout,
                started.generation,
                context,
                error,
            ));
        }

        // Client setup enables external scanning. It is admitted only after
        // the matching device start completion and validated role transition.
        let clients = match connect_clients(
            config,
            minimum_clients,
            context,
            &output.join(format!("cycle-{cycle:02}")),
        ) {
            Ok(clients) => clients,
            Err(error) => {
                return Err(cleanup_after_client_failure(
                    capture,
                    config.timeout,
                    started.generation,
                    context,
                    error,
                ));
            }
        };
        // A real multi-client workload already proves both peers through
        // independently classified transport evidence. The legacy secondary
        // ICMP probe would add an unaccounted third workload and can perturb
        // the exact saturation/fairness interval it is supposed to observe.
        let secondary_probe =
            (!matches!(config.traffic, AccessPointTraffic::UdpMultiClient { .. }))
                .then(|| clients.secondary())
                .flatten()
                .map(|client| {
                    client.spawn_probe(
                        context.lab.access_point.target_address(),
                        traffic_duration(&config.traffic),
                    )
                });
        let primary_link_observation = clients.begin_primary_link_observation();
        let secondary_link_observation = clients.begin_secondary_link_observation();
        let air_output = output.join(format!("cycle-{cycle:02}"));
        let independent_air_capture = if config.capture_independent_laptop_air_monitor {
            fs::create_dir_all(&air_output)?;
            let StationFixtureConfig::OpenWrt(openwrt) = &context.lab.station_fixture else {
                unreachable!("scenario validation constrains independent AP air evidence")
            };
            LocalAirMonitorCapture::start(
                openwrt,
                context.lab.access_point.target_address(),
                traffic_duration(&config.traffic),
                &air_output,
            )
            .map(Some)
        } else {
            Ok(None)
        };
        let data_result = match independent_air_capture.as_ref() {
            Ok(_) => qualify_data_plane(capture, config, context, &clients, &air_output),
            Err(error) => Err(error.to_string().into()),
        };
        let independent_air_result = independent_air_capture
            .and_then(|capture| capture.map(LocalAirMonitorCapture::finish).transpose());
        let primary_link_result =
            primary_link_observation.and_then(clients::PrimaryLinkObservation::finish);
        let secondary_link_result = secondary_link_observation.and_then(|observation| {
            observation
                .map(OpenWrtClientLinkObservation::finish)
                .transpose()
        });
        let secondary_probe_result = secondary_probe.map(|probe| {
            probe.join().map_err(|_| {
                oer_hil_family_ieee80211_fixture::Error::new(
                    "secondary AP client probe thread panicked",
                )
            })?
        });
        // Keep every admitted peer alive until the target has completed its
        // AP stop transaction. Otherwise the clients deauthenticate first and
        // the qualification never exercises the driver's ordered
        // disassociation/deauthentication teardown.
        let stop_result = stop_access_point(capture, config.timeout, started.generation, context);
        let client_restore = restore_clients(clients);
        let mut progress = progress::CycleProgress::new(cycle);
        progress.record("client_restore", &client_restore);
        let probe_air = if config.probe_load {
            oer_hil_family_ieee80211_fixture::probe_load::verify_air(&air_output)
        } else {
            Ok(())
        };
        progress.record("probe_air", &probe_air);
        let client_restore = match (client_restore, probe_air) {
            (Ok(()), result) => result,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(air)) => Err(format!("{error}; probe air evidence: {air}").into()),
        };
        let stop_stack_result = if stop_result.is_ok() {
            report_stack(capture, config.timeout, "ap-stopped")
        } else {
            Ok(())
        };
        let restart_result = if stop_result.is_ok() {
            restore_connected_station(capture, config.timeout, context)
        } else {
            Ok(())
        };

        progress.record("traffic", &data_result);
        progress.record("primary_client_link", &primary_link_result);
        progress.record("secondary_client_link", &secondary_link_result);
        progress.record("independent_air", &independent_air_result);
        if let Some(result) = &secondary_probe_result {
            progress.record("secondary_client_probe", result);
        }
        // Keep intrusive driver observations absent in the performance image.
        // A successful stop is still recorded even without those observations.
        progress.record(
            "access_point_stop",
            &stop_result
                .as_ref()
                .map(|stopped| config.require_driver_observation.then_some(stopped)),
        );
        progress.record("stop_stack", &stop_stack_result);
        progress.record("station_restart", &restart_result);
        progress.save(&context.results);

        if let Err(error) = &data_result {
            let mut data_error = match stop_result.as_ref() {
                Ok(stopped) => format!(
                    "{error}; AP RX hardware: buffer_full={} fifo_overflow={}; AP TX evidence: data={} attempts={} retried_frames={} \
                     maximum_attempts={} minimum_final_rate_kbps={} ack_snr={}/{}/{} \
                     tx_ht40_mcs7={}/{} \
                     ack_timeout_retries={} \
                     cts_timeout_retries={} collision_retries={} hardware_failures={} \
                     hardware_timeouts={} collision_limits={} last_hardware_status={} beacons={}; AP RX evidence: \
                     units={} descriptors={} recycled_descriptors={} retained_descriptors={} discarded_units={} overload_dropped={} critical_reserve={} critical_blocked={} \
                     max_service_us={}/{}/{} data/management/eapol={}/{}/{} dma_total_us/calls={}/{} data_total_us={} \
                     rx_ht40_mcs={:?} rx_ht40_gi_lgi/sgi={}/{} total_ht={} ht_ampdu={} rssi={}/{}/{}/{} protected={} mic_failures={} quarantined={} duplicates={} \
                     radio_rejected={} protocol_rejected={} ethernet_staged={} tcp_staged={} \
                     reorder_buffered/dispatched={}/{} reorder_window_resets={} reorder_gap_timeouts={} \
                     block_ack_requests={} bar_released/missing={}/{}",
                    stopped.rx_hardware.buffer_full,
                    stopped.rx_hardware.fifo_overflow,
                    stopped.data_frames_transmitted,
                    stopped.data_tx_attempts,
                    stopped.data_tx_retried_frames,
                    stopped.data_tx_maximum_attempts,
                    stopped.data_tx_minimum_final_rate_kbps,
                    stopped.data_tx_ack_snr_samples,
                    stopped.data_tx_minimum_ack_snr_db,
                    stopped.data_tx_maximum_ack_snr_db,
                    stopped.tx_ht40_mcs7_aggregates,
                    stopped.tx_ht_aggregates,
                    stopped.tx_ack_timeout_retries,
                    stopped.tx_cts_timeout_retries,
                    stopped.tx_collision_retries,
                    stopped.tx_hardware_failures,
                    stopped.tx_hardware_timeouts,
                    stopped.tx_collision_limits,
                    stopped.tx_last_hardware_status,
                    stopped.beacons_transmitted,
                    stopped.completed_rx_units,
                    stopped.completed_rx_descriptors,
                    stopped.recycled_rx_descriptors,
                    stopped.retained_rx_descriptors,
                    stopped.discarded_rx_units,
                    stopped.rx_overload_discarded_units,
                    stopped.rx_critical_reserve_admissions,
                    stopped.rx_critical_admission_blocked,
                    stopped.maximum_rx_service_micros,
                    stopped.maximum_rx_dma_service_micros,
                    stopped.maximum_rx_protocol_service_micros,
                    stopped.maximum_rx_protected_data_service_micros,
                    stopped.maximum_rx_management_service_micros,
                    stopped.maximum_rx_eapol_service_micros,
                    stopped.total_rx_dma_service_micros,
                    stopped.rx_dma_service_calls,
                    stopped.total_rx_protected_data_service_micros,
                    stopped.rx_ht40_mcs_frames,
                    stopped.rx_ht40_long_gi_frames,
                    stopped.rx_ht40_short_gi_frames,
                    stopped.rx_ht_data_frames,
                    stopped.rx_ht_mpdus_with_aggregation_bit,
                    stopped.rx_rssi_samples,
                    stopped.rx_rssi_sum_dbm,
                    stopped.rx_rssi_min_dbm,
                    stopped.rx_rssi_max_dbm,
                    stopped.protected_data_frames,
                    stopped.rx_mic_failures,
                    stopped.rx_quarantined_frames,
                    stopped.protected_data_duplicates,
                    stopped.protected_data_radio_rejected,
                    stopped.protected_data_protocol_rejected,
                    stopped.ethernet_frames_staged,
                    stopped.ethernet_tcp_frames_staged,
                    stopped.rx_reorder_buffered_mpdus,
                    stopped.rx_reorder_dispatched_mpdus,
                    stopped.rx_reorder_hardware_window_resets,
                    stopped.rx_reorder_gap_timeouts,
                    stopped.rx_block_ack_requests,
                    stopped.rx_reorder_bar_released_mpdus,
                    stopped.rx_reorder_bar_missing_sequences,
                ),
                Err(_) => error.to_string(),
            };
            if let Some(Err(probe_error)) = secondary_probe_result.as_ref() {
                data_error.push_str("; secondary AP client: ");
                data_error.push_str(&probe_error.to_string());
            }
            match &primary_link_result {
                Ok(evidence) => data_error.push_str(&format!("; {evidence}")),
                Err(observation_error) => data_error.push_str(&format!(
                    "; primary AP-client link observation failed: {observation_error}"
                )),
            }
            match &secondary_link_result {
                Ok(Some(evidence)) => data_error.push_str(&format!(
                    "; secondary OpenWrt AP-client link: rx_packets={} rx_bytes={} rx_duration_us={:?} rx_bitrate={:?} tx_packets={} tx_bytes={} tx_bitrate={:?} retries={} failed={} tx_duration_us={} tid0_aqm_drops={}",
                    evidence.rx_packets,
                    evidence.rx_bytes,
                    evidence.rx_duration_micros,
                    evidence.rx_bitrate,
                    evidence.tx_packets,
                    evidence.tx_bytes,
                    evidence.tx_bitrate,
                    evidence.tx_retries,
                    evidence.tx_failed,
                    evidence.tx_duration_micros,
                    evidence.tid0_aqm_drops,
                )),
                Err(observation_error) => data_error.push_str(&format!(
                    "; secondary OpenWrt AP-client link observation failed: {observation_error}"
                )),
                Ok(None) => {}
            }
            if let Err(observation_error) = &independent_air_result {
                data_error.push_str(&format!(
                    "; independent AP air observation failed: {observation_error}"
                ));
            }
            return Err(with_cleanup_errors(
                oer_hil_link::error::with_message(
                    data_error,
                    data_result.err().expect("failed traffic"),
                ),
                client_restore.err(),
                stop_result.as_ref().err().map(|error| error.as_ref()),
                stop_stack_result.err(),
                restart_result.err(),
            ));
        }
        let secondary_client = match secondary_probe_result.transpose() {
            Ok(evidence) => evidence,
            Err(error) => {
                return Err(with_cleanup_errors(
                    error,
                    client_restore.err(),
                    stop_result.as_ref().err().map(|error| error.as_ref()),
                    stop_stack_result.err(),
                    restart_result.err(),
                ));
            }
        };
        let traffic = data_result?;
        let primary_client_link = Some(primary_link_result?);
        let secondary_client_link = secondary_link_result?;
        let independent_air = independent_air_result?;
        client_restore?;
        let stopped = stop_result?;
        stop_stack_result?;
        restart_result?;
        if config.require_driver_observation {
            validate_mcs_evidence(&config.traffic, config.link, &stopped)?;
            validate_access_point_observation(cycle, config.security, minimum_clients, &stopped)?;
        }
        let access_point = config.require_driver_observation.then_some(stopped);
        cycles.push(CycleReport {
            cycle,
            traffic,
            secondary_client,
            primary_client_link,
            secondary_client_link,
            independent_air,
            access_point,
        });
    }
    Ok(())
}

fn validate_access_point_observation(
    cycle: u8,
    security: WifiAccessPointSecurity,
    minimum_clients: u8,
    stopped: &oer_hil_protocol::wifi::WifiAccessPointEvidence,
) -> Result<()> {
    let unacknowledged_disconnects = stopped
        .disassociations_published
        .saturating_sub(stopped.disassociations_acknowledged)
        .saturating_add(
            stopped
                .deauthentications_published
                .saturating_sub(stopped.deauthentications_acknowledged),
        );
    validate_rx_hardware_health(cycle, stopped)?;
    if let Some(retention) = stopped.tx_retention.filter(|evidence| evidence.has_drops()) {
        return Err(format!(
            "AP cycle {cycle}: TX retention dropped admitted frames: {retention:?}"
        )
        .into());
    }
    if stopped.protected_data_protocol_rejected != 0 {
        return Err(format!(
            "AP cycle {cycle}: {} RX protocol rejection(s); first rejection: {:?}",
            stopped.protected_data_protocol_rejected, stopped.first_rx_protocol_rejection,
        )
        .into());
    }
    if stopped.beacons_transmitted == 0
            || stopped.missed_beacon_intervals != 0
            || stopped.maximum_beacon_lateness_micros >= 102_400
            || stopped.authentication_responses == 0
            || stopped.association_responses == 0
            || (security == WifiAccessPointSecurity::Wpa2Personal
                && stopped.wpa2_response_windows < 2)
            || stopped.wpa2_pending_on_stop != 0
            || stopped.wpa2_handshake_failures != 0
            || stopped.wpa2_handshake_timeouts != 0
            || stopped.authorized_peers == 0
            || stopped.maximum_associated_peers == 0
            || stopped.maximum_authorized_peers == 0
            || stopped.maximum_associated_peers < minimum_clients
            || stopped.maximum_authorized_peers < minimum_clients
            || stopped.peer_removals < u32::from(minimum_clients)
            || stopped.disassociations_prepared < u32::from(minimum_clients)
            || stopped.disassociations_published < u32::from(minimum_clients)
            || stopped.disassociations_prepared != stopped.disassociations_published
            || stopped.deauthentications_prepared < u32::from(minimum_clients)
            || stopped.deauthentications_published < u32::from(minimum_clients)
            || stopped.deauthentications_prepared != stopped.deauthentications_published
            // `completed_rx_*` comes from the optional HIL pipeline observer,
            // not from the production AP owner. Functional qualification is
            // already fail-closed on peer authorization, protected protocol
            // RX, hardware-capacity counters and ordered teardown above; it
            // must not require a separately generated diagnostics report.
            // Complete vendor `wifi_softap_stop` submits disassociation and
            // deauthentication back-to-back and does not make peer removal
            // conditional on their ACKs. Our owner waits for each terminal
            // DMA outcome, but an unacknowledged disconnect is expected once
            // the client has already left. No other terminal TX failure is
            // accepted by this gate except explicitly counted one-shot probe
            // ACK misses. Discovery is best-effort; timeout/collision and
            // unclassified failures remain fatal. Saturated totals are unknown.
            || !tx_failures_reconciled(stopped.tx_hardware_failures,
                stopped.tx_probe_ack_timeouts, unacknowledged_disconnects)
            || stopped.tx_hardware_timeouts != 0
            || stopped.tx_collision_limits != 0
            || stopped.control_frames_dropped_while_busy != 0
            || stopped.rx_mic_failures != 0
            || stopped.rx_quarantined_frames != 0
            || stopped.protected_data_radio_rejected != 0
    {
        return Err(
            format!("AP cycle {cycle} lacks peer-visible MAC evidence: {stopped:?}").into(),
        );
    }
    Ok(())
}

fn tx_failures_reconciled(total: u8, probe_ack_timeouts: u8, disconnects: u32) -> bool {
    total != u8::MAX && u32::from(total) == u32::from(probe_ack_timeouts) + disconnects
}

fn validate_rx_hardware_health(
    cycle: u8,
    evidence: &oer_hil_protocol::wifi::WifiAccessPointEvidence,
) -> Result<()> {
    if evidence.rx_hardware.buffer_full != 0 || evidence.rx_hardware.fifo_overflow != 0 {
        return Err(format!(
            "AP cycle {cycle} exhausted hardware RX capacity: buffer_full={} fifo_overflow={}",
            evidence.rx_hardware.buffer_full, evidence.rx_hardware.fifo_overflow,
        )
        .into());
    }
    Ok(())
}

fn validate_mcs_evidence(
    traffic: &AccessPointTraffic,
    expected_link: Option<LinkExpectation>,
    evidence: &oer_hil_protocol::wifi::WifiAccessPointEvidence,
) -> Result<()> {
    let Some(LinkExpectation {
        phy,
        minimum_mcs: Some(minimum_mcs),
        guard_interval,
        ..
    }) = expected_link
    else {
        return Ok(());
    };
    if phy != PhyExpectation::Ht40 || minimum_mcs != 7 {
        return Err(format!(
            "AP target MCS evidence currently supports only HT40 MCS7, requested {} MCS{minimum_mcs}",
            phy.id(),
        )
        .into());
    }
    let Some(direction) = traffic.direction() else {
        return Err("AP minimum_mcs requires a data-plane workload".into());
    };
    let (require_rx, require_tx) = (direction.receives(), direction.transmits());
    if require_rx && evidence.rx_ht40_mcs_frames[7] == 0 {
        return Err(format!(
            "AP client-to-target path did not observe HT40 MCS7 protected data \
             (HT40 MCS histogram={:?}, total HT frames={}, MPDUs with HT-SIG aggregation bit={}; PPDU count/depth unavailable)",
            evidence.rx_ht40_mcs_frames,
            evidence.rx_ht_data_frames,
            evidence.rx_ht_mpdus_with_aggregation_bit,
        )
        .into());
    }
    if require_rx {
        let long = evidence.rx_ht40_long_gi_frames;
        let short = evidence.rx_ht40_short_gi_frames;
        let (expected, unexpected) = match guard_interval {
            HtGuardIntervalExpectation::Any => (1, 0),
            HtGuardIntervalExpectation::Long => (long, short),
            HtGuardIntervalExpectation::Short => (short, long),
        };
        let total = u64::from(long).saturating_add(u64::from(short));
        // AP observations span the whole ownership epoch. A bounded number of
        // association/warm-up data MPDUs can precede the scoped peer rate mask,
        // so require the selected GI to dominate instead of requiring an
        // impossible zero count for the other GI.
        if expected == 0 || u64::from(unexpected).saturating_mul(100) > total {
            return Err(format!(
                "AP client-to-target guard interval mismatch: required={} long={} short={} (required GI must cover at least 99% of observed HT40 data)",
                guard_interval.id(),
                long,
                short,
            )
            .into());
        }
    }
    if require_tx && evidence.tx_ht40_mcs7_aggregates == 0 {
        return Err(format!(
            "AP target-to-client path did not publish an HT40 MCS7 aggregate (HT aggregates={})",
            evidence.tx_ht_aggregates,
        )
        .into());
    }
    Ok(())
}

fn traffic_duration(traffic: &AccessPointTraffic) -> Duration {
    let seconds = |value: u16| Duration::from_secs(u64::from(value));
    match traffic {
        AccessPointTraffic::None {} => Duration::from_secs(2),
        AccessPointTraffic::Icmp(icmp) => {
            Duration::from_millis(u64::from(icmp.count) * u64::from(icmp.interval_ms))
                .max(Duration::from_secs(2))
        }
        AccessPointTraffic::Udp(udp) => seconds(udp.duration_seconds),
        AccessPointTraffic::UdpMultiClient(udp) => seconds(udp.duration_seconds),
        AccessPointTraffic::Tcp(tcp) => seconds(tcp.duration_seconds),
    }
}

fn qualify_data_plane(
    capture: &SerialCapture,
    config: &Config,
    context: &Context<'_>,
    clients: &ConnectedClients,
    output: &Path,
) -> Result<TrafficReport> {
    let target = context.lab.access_point.target_address();
    let traffic_target = clients.traffic_target(target)?;
    match &config.traffic {
        AccessPointTraffic::None {} => Ok(TrafficReport::None),
        AccessPointTraffic::Icmp(icmp) => {
            qualify_icmp(traffic_target, clients.openwrt_primary().is_none(), icmp)
        }
        AccessPointTraffic::Udp(udp) => qualify_udp(
            output,
            capture,
            config,
            context,
            clients,
            UdpWorkload {
                direction: udp.offer.direction(),
                duration: Duration::from_secs(u64::from(udp.duration_seconds)),
                rx_rate_bps: udp.offer.rx_bps,
                tx_rate_bps: udp.offer.tx_bps,
                secondary_rx_rate_bps: None,
                secondary_tx_rate_bps: None,
                secondary_tx_pacing_group_datagrams: None,
                payload_bytes: usize::from(udp.payload_bytes),
            },
            udp.criteria,
        ),
        AccessPointTraffic::UdpMultiClient(udp) => qualify_multi_client_udp(
            output,
            capture,
            config,
            context,
            clients,
            UdpWorkload {
                direction: udp.offer.direction(),
                duration: Duration::from_secs(u64::from(udp.duration_seconds)),
                rx_rate_bps: udp.offer.rx_bps,
                tx_rate_bps: udp.offer.tx_bps,
                secondary_rx_rate_bps: udp.secondary.rx_bps,
                secondary_tx_rate_bps: udp.secondary.tx_bps,
                secondary_tx_pacing_group_datagrams: udp.secondary.tx_pacing_group_datagrams,
                payload_bytes: usize::from(udp.payload_bytes),
            },
            udp.criteria,
        ),
        AccessPointTraffic::Tcp(tcp) => qualify_tcp(
            capture,
            config,
            &context.measurements,
            context.lab.access_point.target_address(),
            TcpWorkload {
                direction: tcp.offer.direction(),
                duration: Duration::from_secs(u64::from(tcp.duration_seconds)),
                rx_rate_bps: tcp.offer.rx_bps,
                tx_rate_bps: tcp.offer.tx_bps,
                chunk_bytes: usize::from(tcp.chunk_bytes),
            },
            tcp.criteria.floors(),
        ),
    }
}

fn session_report(direction: Direction, evidence: &SessionEvidence) -> SessionReport {
    SessionReport {
        direction,
        rx_bytes: evidence.transport.rx_bytes,
        tx_bytes: evidence.transport.tx_bytes,
        rx_units: evidence.transport.rx_units,
        tx_units: evidence.transport.tx_units,
        elapsed_micros: evidence.transport.elapsed_micros,
    }
}

/// Record the AP's aggregate rates with their floors, then check them.
///
/// The measurements carry the verdict into `runs why` and let `perf` baseline
/// the AP workload like the station ones.
fn validate_rate_criteria(
    report: &SessionReport,
    criteria: RateFloors,
    measurements: &oer_hil_workload::measurements::Recorder,
) -> Result<()> {
    if report.elapsed_micros == 0 {
        return Err("AP transport reported zero elapsed time".into());
    }
    let bitrate = |bytes: u64| {
        u128::from(bytes)
            .saturating_mul(8_000_000)
            .checked_div(u128::from(report.elapsed_micros))
            .unwrap_or(0)
    };
    let rate = |bytes: u64| u64::try_from(bitrate(bytes)).unwrap_or(u64::MAX);
    if report.rx_bytes != 0 || criteria.minimum_rx_bps.is_some() {
        measurements.rate(
            "ap.rx.aggregate-rate",
            rate(report.rx_bytes),
            criteria.minimum_rx_bps,
        );
    }
    if report.tx_bytes != 0 || criteria.minimum_tx_bps.is_some() {
        measurements.rate(
            "ap.tx.aggregate-rate",
            rate(report.tx_bytes),
            criteria.minimum_tx_bps,
        );
    }
    if let Some(minimum) = criteria.minimum_combined_bps {
        measurements.rate(
            "ap.combined-rate",
            rate(report.rx_bytes).saturating_add(rate(report.tx_bytes)),
            Some(minimum),
        );
    }
    if let Some(minimum) = criteria.minimum_rx_bps
        && bitrate(report.rx_bytes) < u128::from(minimum)
    {
        return Err(format!(
            "AP RX bitrate {} is below required {minimum}",
            bitrate(report.rx_bytes)
        )
        .into());
    }
    if let Some(minimum) = criteria.minimum_tx_bps
        && bitrate(report.tx_bytes) < u128::from(minimum)
    {
        return Err(format!(
            "AP TX bitrate {} is below required {minimum}",
            bitrate(report.tx_bytes)
        )
        .into());
    }
    if let Some(minimum) = criteria.minimum_combined_bps {
        let combined = bitrate(report.rx_bytes).saturating_add(bitrate(report.tx_bytes));
        if combined < u128::from(minimum) {
            return Err(
                format!("AP combined bitrate {combined} is below required {minimum}").into(),
            );
        }
    }
    Ok(())
}

fn cleanup_after_client_failure(
    capture: &SerialCapture,
    timeout: Duration,
    generation: u32,
    context: &Context<'_>,
    primary: Box<dyn std::error::Error + Send + Sync>,
) -> Box<dyn std::error::Error + Send + Sync> {
    let stop = stop_access_point(capture, timeout, generation, context);
    let stop_stack = if stop.is_ok() {
        report_stack(capture, timeout, "ap-stopped")
    } else {
        Ok(())
    };
    let restart = if stop.is_ok() {
        restore_connected_station(capture, timeout, context)
    } else {
        Ok(())
    };
    with_cleanup_errors(
        primary,
        None,
        stop.as_ref().err().map(|error| error.as_ref()),
        stop_stack.err(),
        restart.err(),
    )
}

fn restore_connected_station(
    capture: &SerialCapture,
    timeout: Duration,
    context: &Context<'_>,
) -> Result<()> {
    let lifecycle_cursor = capture.station_lifecycle_cursor();
    start_station(capture, context, timeout)?;
    capture.wait_for_connected_station_after(lifecycle_cursor, timeout)?;
    report_stack(capture, timeout, "ap-station-reconnected")
}

fn with_cleanup_errors(
    primary: Box<dyn std::error::Error + Send + Sync>,
    client: Option<Box<dyn std::error::Error + Send + Sync>>,
    stop: Option<&(dyn std::error::Error + Send + Sync)>,
    stop_stack: Option<Box<dyn std::error::Error + Send + Sync>>,
    restart: Option<Box<dyn std::error::Error + Send + Sync>>,
) -> Box<dyn std::error::Error + Send + Sync> {
    let mut message = primary.to_string();
    if let Some(error) = client {
        message.push_str(&format!("; client restore failed: {error}"));
    }
    if let Some(error) = stop {
        message.push_str(&format!("; AP stop failed: {error}"));
    }
    if let Some(error) = stop_stack {
        message.push_str(&format!("; stopped-AP stack query failed: {error}"));
    }
    if let Some(error) = restart {
        message.push_str(&format!("; station restore failed: {error}"));
    }
    oer_hil_link::error::with_message(message, primary)
}

fn stop_access_point(
    capture: &SerialCapture,
    timeout: Duration,
    generation: u32,
    context: &Context<'_>,
) -> Result<oer_hil_protocol::wifi::WifiAccessPointEvidence> {
    let handle = capture.command(
        oer_hil_protocol::wifi::StopAccessPoint,
        oer_hil_link::PROTOCOL_READY_TIMEOUT,
    )?;
    let evidence = capture
        .wait_command::<oer_hil_protocol::wifi::AccessPointStopped>(handle, timeout)
        .map(|completed| completed.0)?;
    if context.settings.ap_scheduler != oer_hil_protocol::wifi::WifiApScheduler::Disabled {
        capture.require_access_point_airtime(handle)?;
    }
    if evidence.generation != generation
        || evidence.channel != context.lab.access_point.channel()
        || evidence.bandwidth_mhz != context.lab.access_point.bandwidth_mhz()
    {
        return Err(format!("AP returned inconsistent stop evidence: {evidence:?}").into());
    }
    Ok(evidence)
}

const fn protocol_direction(direction: Direction) -> ProtocolDirection {
    match direction {
        Direction::Rx => ProtocolDirection::Rx,
        Direction::Tx => ProtocolDirection::Tx,
        Direction::Bidirectional => ProtocolDirection::Bidirectional,
    }
}

#[cfg(test)]
mod tests;
