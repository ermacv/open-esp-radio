//! Host qualification for explicit Wi-Fi role ownership transitions.

use crate::link::WifiCapture as _;
use oer_hil_workload::context::Context;
use std::{fs, path::Path, time::Duration};

use oer_hil_protocol::{
    wifi::StationDisconnectReason, wifi::StationLifecycleEvent, wifi::WifiMonitorRequest,
    wifi::WifiNetworkInterface, wifi::WifiRadioRestartEvidence, wifi::WifiRadioRestartRf,
    wifi::WifiRole, wifi::WifiRoleTransitionEvidence, wifi::WifiScanEvidence,
    wifi::WifiScanRequest, wifi::WifiStationAccessPointRequest,
};

use crate::{Result, scenario::LinkExpectation};
use oer_hil_link::{SerialCapture, StationConnectionObservation};
use oer_hil_scenario_catalog::link::PhyExpectation;

/// One Wi-Fi role ownership transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Stop,
    Start,
    Restart,
    Scan,
    Monitor,
    AccessPoint,
    StationAccessPoint,
    Roundtrip,
}

impl Operation {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Start => "start",
            Self::Restart => "restart",
            Self::Scan => "scan",
            Self::Monitor => "monitor",
            Self::AccessPoint => "ap",
            Self::StationAccessPoint => "sta-ap",
            Self::Roundtrip => "roundtrip",
        }
    }
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_MONITOR_DURATION: Duration = Duration::from_secs(3);
const DEFAULT_SCAN_DWELL_MILLIS: u16 = 200;
pub(super) mod data_path;

pub struct Config {
    pub timeout: Duration,
    pub restart_cycles: u8,
    /// What each restart must do to shared RF.
    pub restart_rf: WifiRadioRestartRf,
    pub monitor_duration: Duration,
    pub monitor_channel: Option<u8>,
    pub snapshot_length: u16,
}

pub fn run(
    operation: Operation,
    options: Config,
    output: &Path,
    context: &Context<'_>,
    link: LinkExpectation,
) -> Result<()> {
    let options = options.validate()?;

    fs::create_dir_all(output)?;
    context.with_capture(output, |capture| {
        qualify(capture, context, operation, &options, link)
    })?;
    eprintln!("wifi_{}=PASS", operation.id());
    eprintln!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(
    capture: &SerialCapture,
    context: &Context<'_>,
    operation: Operation,
    options: &Config,
    link: LinkExpectation,
) -> Result<()> {
    let capabilities = capture.prepare_station(context.target(), options.timeout)?;
    if !capabilities.has::<oer_hil_protocol::wifi::RoleControl>() {
        return Err("firmware does not advertise explicit Wi-Fi role control".into());
    }
    report_stack(capture, options.timeout, "connected")?;

    if operation == Operation::Restart {
        context.lab.station_fixture.require_phy(link.phy)?;
        if context.settings.data_plane
            != oer_hil_protocol::wifi::WifiDataPlanePlacement::SplitRadioNetwork
        {
            return Err("radio lifecycle requires split radio/network data-plane ownership".into());
        }
        let initial = capture.wait_for_connected_station_link_after(0, options.timeout)?;
        require_station_link(initial, link)?;
        data_path::prove_station_data_path(
            capture,
            context,
            options.timeout,
            "before-cycle",
            initial.event_cursor_after,
        )?;
        let mut station_link_generation = initial.generation;
        let mut stopped =
            stop_station_for_lifecycle(capture, options.timeout, station_link_generation)?;
        report_stack(capture, options.timeout, "station-stopped")?;
        for cycle in 1..=options.restart_cycles {
            let restarted = capture
                .wait_command::<oer_hil_protocol::wifi::RadioRestarted>(
                    capture.command(
                        oer_hil_protocol::wifi::RestartRadio,
                        oer_hil_link::PROTOCOL_READY_TIMEOUT,
                    )?,
                    options.timeout,
                )
                .map(|completed| completed.0)?;
            require_radio_restart(stopped, restarted, options.restart_rf)?;
            eprintln!(
                "wifi_radio_restart_cycle={cycle} generation={} rf={:?}",
                restarted.generation, restarted.rf,
            );
            let lifecycle_generation = restarted.generation;
            report_stack(capture, options.timeout, "radio-restarted")?;
            let (started, connected) =
                start_connected_station(capture, context, options.timeout, link)?;
            require_station_after_lifecycle(lifecycle_generation, started)?;
            let expected_link_generation = station_link_generation.wrapping_add(1);
            if connected.generation != expected_link_generation {
                return Err(format!(
                    "station connected generation {} after lifecycle stop of generation {}, expected {}",
                    connected.generation, station_link_generation, expected_link_generation,
                ).into());
            }
            station_link_generation = connected.generation;
            data_path::prove_station_data_path(
                capture,
                context,
                options.timeout,
                "after-cycle",
                connected.event_cursor_after,
            )?;
            if cycle < options.restart_cycles {
                stopped =
                    stop_station_for_lifecycle(capture, options.timeout, station_link_generation)?;
            }
        }
        report_stack(
            capture,
            options.timeout,
            "station-connected-after-radio-restart",
        )?;
        return Ok(());
    }

    let stopped = stop_station(capture, options.timeout)?;
    report_stack(capture, options.timeout, "station-stopped")?;
    eprintln!("wifi_station_stopped_generation={}", stopped.generation);
    if operation == Operation::Stop {
        return Ok(());
    }
    if operation == Operation::Start {
        start_station(capture, context, options.timeout)?;
        report_stack(capture, options.timeout, "station-started")?;
        return Ok(());
    }
    if operation == Operation::AccessPoint {
        if !capabilities.has::<oer_hil_protocol::wifi::AccessPoint>() {
            return Err("firmware does not advertise the access-point role".into());
        }
        let mut request = context
            .lab
            .access_point
            .protocol_request(oer_hil_protocol::wifi::WifiAccessPointSecurity::Wpa2Personal)?;
        if let Some(channel) = options.monitor_channel {
            request.channel = channel;
        }
        let channel = request.channel;
        let bandwidth_mhz = request.channel_width.bandwidth_mhz();
        let started = capture
            .wait_command::<oer_hil_protocol::wifi::AccessPointStarted>(
                capture.command(
                    oer_hil_protocol::wifi::StartAccessPoint(request),
                    oer_hil_link::PROTOCOL_READY_TIMEOUT,
                )?,
                options.timeout,
            )
            .map(|completed| completed.0)?;
        require_transition(started, WifiRole::Idle, WifiRole::AccessPoint)?;
        report_stack(capture, options.timeout, "access-point-started")?;
        oer_process::sleep(options.monitor_duration)?;
        let stopped = capture
            .wait_command::<oer_hil_protocol::wifi::AccessPointStopped>(
                capture.command(
                    oer_hil_protocol::wifi::StopAccessPoint,
                    oer_hil_link::PROTOCOL_READY_TIMEOUT,
                )?,
                options.timeout,
            )
            .map(|completed| completed.0)?;
        if stopped.generation != started.generation
            || stopped.channel != channel
            || stopped.bandwidth_mhz != bandwidth_mhz
            || stopped.beacons_transmitted == 0
        {
            return Err(format!("access point returned inconsistent evidence: {stopped:?}").into());
        }
        eprintln!(
            "wifi_ap_generation={} channel={} bandwidth_mhz={} beacons={} auth_responses={} assoc_responses={} authorizations={} max_associated={} max_authorized={} peer_removals={} auth_timeouts={} wpa2_windows={} wpa2_pending_on_stop={} wpa2_retries={} wpa2_failures={} wpa2_timeouts={} inactivity_timeouts={} disassoc_prepared={} disassoc_published={} disassoc_acked={} deauth_prepared={} deauth_published={} deauth_acked={} rx_units={} rx_descriptors={} recycled_rx_descriptors={} retained_rx_descriptors={} hardware_buffer_full={} hardware_fifo_overflow={} discarded_rx_units={} overload_dropped={} critical_reserve={} critical_blocked={} ignored_rx={} control_staged={} control_busy_drops={} ethernet_staged={} network_tx_rejected={} data_tx={} data_tx_attempts={} data_tx_retried={} data_tx_max_attempts={} data_tx_min_rate_kbps={} data_tx_ack_snr={}/{}/{} tx_hardware_failures={} tx_probe_ack_timeouts={} tx_hardware_timeouts={} tx_collision_limits={} tx_last_hardware_status={}",
            stopped.generation,
            stopped.channel,
            stopped.bandwidth_mhz,
            stopped.beacons_transmitted,
            stopped.authentication_responses,
            stopped.association_responses,
            stopped.authorized_peers,
            stopped.maximum_associated_peers,
            stopped.maximum_authorized_peers,
            stopped.peer_removals,
            stopped.authentication_timeouts,
            stopped.wpa2_response_windows,
            stopped.wpa2_pending_on_stop,
            stopped.wpa2_retransmissions,
            stopped.wpa2_handshake_failures,
            stopped.wpa2_handshake_timeouts,
            stopped.inactivity_timeouts,
            stopped.disassociations_prepared,
            stopped.disassociations_published,
            stopped.disassociations_acknowledged,
            stopped.deauthentications_prepared,
            stopped.deauthentications_published,
            stopped.deauthentications_acknowledged,
            stopped.completed_rx_units,
            stopped.completed_rx_descriptors,
            stopped.recycled_rx_descriptors,
            stopped.retained_rx_descriptors,
            stopped.rx_hardware.buffer_full,
            stopped.rx_hardware.fifo_overflow,
            stopped.discarded_rx_units,
            stopped.rx_overload_discarded_units,
            stopped.rx_critical_reserve_admissions,
            stopped.rx_critical_admission_blocked,
            stopped.ignored_rx_frames,
            stopped.control_frames_staged,
            stopped.control_frames_dropped_while_busy,
            stopped.ethernet_frames_staged,
            stopped.network_tx_frames_rejected,
            stopped.data_frames_transmitted,
            stopped.data_tx_attempts,
            stopped.data_tx_retried_frames,
            stopped.data_tx_maximum_attempts,
            stopped.data_tx_minimum_final_rate_kbps,
            stopped.data_tx_ack_snr_samples,
            stopped.data_tx_minimum_ack_snr_db,
            stopped.data_tx_maximum_ack_snr_db,
            stopped.tx_hardware_failures,
            stopped.tx_probe_ack_timeouts,
            stopped.tx_hardware_timeouts,
            stopped.tx_collision_limits,
            stopped.tx_last_hardware_status,
        );
        start_station(capture, context, options.timeout)?;
        report_stack(capture, options.timeout, "station-restarted")?;
        return Ok(());
    }

    if operation == Operation::StationAccessPoint {
        if !capabilities.has::<oer_hil_protocol::wifi::StationAccessPoint>() {
            return Err("firmware does not advertise simultaneous STA+AP".into());
        }
        let mut access_point = context
            .lab
            .access_point
            .protocol_request(oer_hil_protocol::wifi::WifiAccessPointSecurity::Wpa2Personal)?;
        if let Some(channel) = options.monitor_channel {
            access_point.channel = channel;
        }
        let request = WifiStationAccessPointRequest {
            station_credentials: context.lab.station.protocol_credentials()?,
            access_point,
        };
        let started = capture
            .wait_command::<oer_hil_protocol::wifi::RoleTransitioned>(
                capture.command(
                    oer_hil_protocol::wifi::StartStationAccessPoint(request),
                    oer_hil_link::PROTOCOL_READY_TIMEOUT,
                )?,
                options.timeout,
            )
            .map(|completed| completed.0)?;
        require_transition(started, WifiRole::Idle, WifiRole::StationAccessPoint)?;
        let deadline = std::time::Instant::now() + options.timeout;
        while std::time::Instant::now() < deadline {
            oer_process::check_cancelled()?;
            let station = capture.observed_protocol_ipv4(WifiNetworkInterface::Station);
            let access_point = capture.observed_protocol_ipv4(WifiNetworkInterface::AccessPoint);
            if let (Some(station), Some(access_point)) = (station, access_point) {
                eprintln!("wifi_sta_ap_station_ipv4={station}");
                eprintln!("wifi_sta_ap_access_point_ipv4={access_point}");
                break;
            }
            oer_process::sleep(Duration::from_millis(20))?;
        }
        if capture
            .observed_protocol_ipv4(WifiNetworkInterface::Station)
            .is_none()
            || capture
                .observed_protocol_ipv4(WifiNetworkInterface::AccessPoint)
                .is_none()
        {
            return Err("paired role did not publish both network endpoints".into());
        }
        oer_process::sleep(options.monitor_duration)?;
        let stopped = capture
            .wait_command::<oer_hil_protocol::wifi::StationAccessPointStopped>(
                capture.command(
                    oer_hil_protocol::wifi::StopStationAccessPoint,
                    oer_hil_link::PROTOCOL_READY_TIMEOUT,
                )?,
                options.timeout,
            )
            .map(|completed| completed.0)?;
        require_transition(
            stopped.transition,
            WifiRole::StationAccessPoint,
            WifiRole::Idle,
        )?;
        if stopped.transition.generation != started.generation {
            return Err("paired start/stop generations differ".into());
        }
        eprintln!(
            "wifi_sta_ap_beacons={} missed={} maximum_lateness_micros={} buffer_full={} fifo_overflow={}",
            stopped.access_point.beacons_transmitted,
            stopped.access_point.missed_beacon_intervals,
            stopped.access_point.maximum_beacon_lateness_micros,
            stopped.access_point.rx_hardware.buffer_full,
            stopped.access_point.rx_hardware.fifo_overflow,
        );
        return Ok(());
    }

    let scan = scan(capture, options.timeout)?;
    report_stack(capture, options.timeout, "scan-complete")?;
    eprintln!(
        "wifi_scan_generation={} elapsed_micros={} observed_frames={} unique_bss={} configured_ssid_channel={} configured_ssid_rssi_dbm={}",
        scan.generation,
        scan.elapsed_micros,
        scan.observed_frames,
        scan.unique_bss,
        scan.configured_ssid_channel,
        scan.configured_ssid_rssi_dbm,
    );
    if operation == Operation::Scan {
        start_station(capture, context, options.timeout)?;
        report_stack(capture, options.timeout, "station-restarted")?;
        return Ok(());
    }

    let channel = options
        .monitor_channel
        .unwrap_or(scan.configured_ssid_channel);
    if !(1..=13).contains(&channel) {
        return Err("scan did not provide a valid configured-network channel".into());
    }
    let started = capture
        .wait_command::<oer_hil_protocol::wifi::MonitorStarted>(
            capture.command(
                oer_hil_protocol::wifi::StartMonitor(WifiMonitorRequest {
                    channel,
                    snapshot_length: options.snapshot_length,
                }),
                oer_hil_link::PROTOCOL_READY_TIMEOUT,
            )?,
            options.timeout,
        )
        .map(|completed| completed.0)?;
    require_transition(started, WifiRole::Idle, WifiRole::Monitor)?;
    oer_process::sleep(options.monitor_duration)?;
    let stopped = capture
        .wait_command::<oer_hil_protocol::wifi::MonitorStopped>(
            capture.command(
                oer_hil_protocol::wifi::StopMonitor,
                oer_hil_link::PROTOCOL_READY_TIMEOUT,
            )?,
            options.timeout,
        )
        .map(|completed| completed.0)?;
    report_stack(capture, options.timeout, "monitor-stopped")?;
    if stopped.generation != started.generation
        || stopped.channel != channel
        || stopped.generation_mismatches != 0
        || stopped.channel_mismatches != 0
    {
        return Err(format!("monitor returned inconsistent evidence: {stopped:?}").into());
    }
    if stopped.captured_frames == 0 || stopped.captured_bytes == 0 {
        return Err(format!("monitor epoch captured no frames: {stopped:?}").into());
    }
    validate_elapsed(
        "monitor lifecycle",
        stopped.elapsed_micros,
        options.monitor_duration,
        150,
    )?;
    eprintln!(
        "wifi_monitor_generation={} elapsed_micros={} channel={} captured_frames={} captured_bytes={} channel_unavailable={} last_observed_channel={}",
        stopped.generation,
        stopped.elapsed_micros,
        stopped.channel,
        stopped.captured_frames,
        stopped.captured_bytes,
        stopped.channel_unavailable,
        stopped.last_observed_channel,
    );
    start_station(capture, context, options.timeout)?;
    report_stack(capture, options.timeout, "station-restarted")?;
    Ok(())
}

pub fn report_stack(capture: &SerialCapture, timeout: Duration, stage: &str) -> Result<()> {
    let usage = capture.query_stack_usage(timeout)?;
    eprintln!(
        "stack_stage={stage} cpu0_free={}/{} cpu0_required={} cpu1_free={}/{} cpu1_required={}",
        usage.cpu0.free_bytes,
        usage.cpu0.capacity_bytes,
        usage.cpu0.minimum_free_bytes,
        usage.cpu1.free_bytes,
        usage.cpu1.capacity_bytes,
        usage.cpu1.minimum_free_bytes,
    );
    Ok(())
}

pub fn stop_station(
    capture: &SerialCapture,
    timeout: Duration,
) -> Result<WifiRoleTransitionEvidence> {
    let evidence = capture
        .wait_command::<oer_hil_protocol::wifi::RoleTransitioned>(
            capture.command(
                oer_hil_protocol::wifi::StopStation,
                oer_hil_link::PROTOCOL_READY_TIMEOUT,
            )?,
            timeout,
        )
        .map(|completed| completed.0)?;
    require_transition(evidence, WifiRole::Station, WifiRole::Idle)?;
    Ok(evidence)
}

fn stop_station_for_lifecycle(
    capture: &SerialCapture,
    timeout: Duration,
    connected_generation: u32,
) -> Result<WifiRoleTransitionEvidence> {
    let lifecycle_cursor = capture.station_lifecycle_cursor();
    let stopped = stop_station(capture, timeout)?;
    let mut cursor = lifecycle_cursor;
    let disconnected = capture.wait_station_lifecycle_event(&mut cursor, timeout)?;
    require_lifecycle_disconnect(disconnected, connected_generation)?;
    Ok(stopped)
}

fn require_lifecycle_disconnect(event: StationLifecycleEvent, generation: u32) -> Result<()> {
    if event
        != (StationLifecycleEvent::Disconnected {
            generation,
            reason: StationDisconnectReason::LinkPolicy,
        })
    {
        return Err(format!(
            "station stop did not publish LinkPolicy disconnect for connected generation {generation}: {event:?}"
        ).into());
    }
    Ok(())
}

pub fn start_station(
    capture: &SerialCapture,
    context: &Context<'_>,
    timeout: Duration,
) -> Result<()> {
    start_station_evidence(capture, context, timeout).map(|_| ())
}

fn start_station_evidence(
    capture: &SerialCapture,
    context: &Context<'_>,
    timeout: Duration,
) -> Result<WifiRoleTransitionEvidence> {
    let evidence = capture
        .wait_command::<oer_hil_protocol::wifi::RoleTransitioned>(
            capture.command(
                oer_hil_protocol::wifi::StartStation(oer_hil_protocol::wifi::StationStart {
                    credentials: context.target().station.credentials()?,
                    power_save: context.target().settings.station_power_save,
                }),
                oer_hil_link::PROTOCOL_READY_TIMEOUT,
            )?,
            timeout,
        )
        .map(|completed| completed.0)?;
    require_transition(evidence, WifiRole::Idle, WifiRole::Station)?;
    Ok(evidence)
}

fn start_connected_station(
    capture: &SerialCapture,
    context: &Context<'_>,
    timeout: Duration,
    link: LinkExpectation,
) -> Result<(WifiRoleTransitionEvidence, StationConnectionObservation)> {
    let lifecycle_cursor = capture.station_lifecycle_cursor();
    let evidence = start_station_evidence(capture, context, timeout)?;
    let connected = capture.wait_for_connected_station_link_after(lifecycle_cursor, timeout)?;
    require_station_link(connected, link)?;
    Ok((evidence, connected))
}

pub(super) fn require_station_link(
    connected: StationConnectionObservation,
    link: LinkExpectation,
) -> Result<()> {
    let expected_bandwidth = match link.phy {
        PhyExpectation::Ht40 => 40,
        PhyExpectation::Legacy | PhyExpectation::Ht20 | PhyExpectation::He20 => 20,
    };
    let expected_security = if link.access_point_security.offers_sae() {
        oer_hil_protocol::wifi::StationLinkSecurity::Wpa3Personal
    } else {
        oer_hil_protocol::wifi::StationLinkSecurity::Wpa2Personal {
            management_protection: link.management_frame_protection.negotiated(),
        }
    };
    if connected.association_bandwidth_mhz != Some(expected_bandwidth)
        || connected.security != Some(expected_security)
    {
        return Err(format!(
            "station connected generation {} with negotiated width {:?} MHz/security {:?}, expected {} MHz/{:?}",
            connected.generation,
            connected.association_bandwidth_mhz,
            connected.security,
            expected_bandwidth,
            expected_security,
        ).into());
    }
    Ok(())
}

pub fn scan(capture: &SerialCapture, timeout: Duration) -> Result<WifiScanEvidence> {
    let request = WifiScanRequest {
        channel_mask_2_4_ghz: 0x1fff,
        dwell_millis: DEFAULT_SCAN_DWELL_MILLIS,
    };
    let evidence = capture
        .wait_command::<oer_hil_protocol::wifi::ScanCompleted>(
            capture.command(
                oer_hil_protocol::wifi::Scan(request),
                oer_hil_link::PROTOCOL_READY_TIMEOUT,
            )?,
            timeout,
        )
        .map(|completed| completed.0)?;
    if !evidence.configured_ssid_found {
        return Err(format!("scan did not find the configured SSID: {evidence:?}").into());
    }
    let expected = Duration::from_millis(
        u64::from(request.dwell_millis) * u64::from(request.channel_mask_2_4_ghz.count_ones()),
    );
    validate_elapsed("standalone scan", evidence.elapsed_micros, expected, 150)?;
    Ok(evidence)
}

fn validate_elapsed(
    operation: &str,
    observed_micros: u64,
    expected: Duration,
    maximum_percent: u64,
) -> Result<()> {
    let expected_micros = expected.as_micros().min(u128::from(u64::MAX)) as u64;
    let minimum = expected_micros * 95 / 100;
    let maximum = expected_micros * maximum_percent / 100;
    if observed_micros < minimum || observed_micros > maximum {
        return Err(format!(
            "{operation} timing is outside the qualified range: expected_us={expected_micros} observed_us={observed_micros} range={minimum}..={maximum}"
        )
        .into());
    }
    Ok(())
}

pub fn require_transition(
    evidence: WifiRoleTransitionEvidence,
    previous: WifiRole,
    current: WifiRole,
) -> Result<()> {
    if evidence.previous != previous || evidence.current != current {
        return Err(format!(
            "unexpected Wi-Fi role transition: expected {previous:?}->{current:?}, got {evidence:?}"
        )
        .into());
    }
    Ok(())
}

fn require_station_after_lifecycle(
    lifecycle_generation: u32,
    started: WifiRoleTransitionEvidence,
) -> Result<()> {
    require_transition(started, WifiRole::Idle, WifiRole::Station)?;
    let expected_generation = lifecycle_generation.wrapping_add(1);
    if started.generation != expected_generation {
        return Err(format!(
            "station after radio lifecycle used generation {}, expected {} after lifecycle generation {}",
            started.generation, expected_generation, lifecycle_generation,
        )
        .into());
    }
    Ok(())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            restart_cycles: 1,
            restart_rf: WifiRadioRestartRf::ClosedAndWoken,
            monitor_duration: DEFAULT_MONITOR_DURATION,
            monitor_channel: None,
            snapshot_length: 256,
        }
    }
}

fn require_radio_restart(
    stopped: WifiRoleTransitionEvidence,
    restarted: WifiRadioRestartEvidence,
    expected_rf: WifiRadioRestartRf,
) -> Result<()> {
    let expected_generation = stopped.generation.wrapping_add(1);
    if restarted.generation != expected_generation {
        return Err(format!(
            "idle radio restart returned generation {}, expected {} after stopped generation {}",
            restarted.generation, expected_generation, stopped.generation,
        )
        .into());
    }
    // As the last PHY client Wi-Fi's release closes RF and the rejoin wakes
    // it; beside another client RF stays open.
    if restarted.rf != expected_rf {
        return Err(format!(
            "idle radio restart left RF {:?}, expected {expected_rf:?}",
            restarted.rf,
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;

impl Config {
    fn validate(self) -> Result<Self> {
        if !(Duration::from_secs(10)..=Duration::from_secs(180)).contains(&self.timeout) {
            return Err("role timeout must be in 10..=180 seconds".into());
        }
        if !(Duration::from_secs(1)..=Duration::from_secs(30)).contains(&self.monitor_duration) {
            return Err("monitor duration must be in 1..=30 seconds".into());
        }
        if self
            .monitor_channel
            .is_some_and(|channel| !(1..=13).contains(&channel))
        {
            return Err("monitor channel must be in 1..=13".into());
        }
        if self.snapshot_length > 2304 {
            return Err("snapshot length must be at most 2304".into());
        }
        Ok(self)
    }
}
