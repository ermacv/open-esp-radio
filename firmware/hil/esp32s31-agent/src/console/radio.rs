//! Radio application side of the shared HIL console.
//!
//! Command admission remains in the console; these endpoints are compiled only
//! for images that contain a radio/network product owner.

use super::*;
#[cfg(all(feature = "i-radio", not(feature = "i-thread")))]
use embassy_futures::select::{Either3, select3};
use oer_tool_data_hil_protocol::{
    phy::STARTUP_ARTIFACT_CHUNK_MAX_LEN, phy::StartupArtifactDisposition,
    phy::StartupArtifactStatus, wifi::StationEpochEvidence, wifi::StationLifecycleEvent,
    wifi::WifiAccessPointEvidence, wifi::WifiMonitorEvidence, wifi::WifiMonitorFrameChunk,
    wifi::WifiRoleFailureEvidence, wifi::WifiRoleTransitionEvidence, wifi::WifiScanEvidence,
    wifi::WifiStationAccessPointStopEvidence,
};

/// The single owner-consuming operation selected before radio initialization.
///
/// The IEEE 802.15.4 variant is a bounded diagnostic discriminator only. It
/// neither enables the CPU interrupt route nor attests production IRQ
/// readiness, and the target returns after publishing its one completion.
#[allow(
    clippy::large_enum_variant,
    reason = "the no-alloc target moves the existing bounded startup artifact through this one-shot owner handoff"
)]
pub enum PreInitializationRequest {
    Startup(StartupConfiguration),
    #[cfg(feature = "i-event-status-probe")]
    Ieee802154EventStatus(Ieee802154EventStatusProbe),
    #[cfg(feature = "i-ed-event-probe")]
    Ieee802154EdEvent(Ieee802154EdEventProbe),
    #[cfg(feature = "i-route-probe")]
    Ieee802154Route(Ieee802154RouteProbe),
    #[cfg(feature = "i-radio")]
    Ieee802154AirCheck(Ieee802154AirCheck),
    #[cfg(feature = "i-radio")]
    Ieee802154Session(Ieee802154SessionStart),
    #[cfg(feature = "i-thread")]
    Ieee802154Thread(Ieee802154ThreadStart),
}

/// Queues one best-effort diagnostic line on the runtime USB transport.
///
/// This path is serialized by the console task with binary protocol frames. Runtime code must use this function so a ROM write
/// cannot overtake a USB packet that the asynchronous HAL has only submitted.
pub fn runtime_log(args: Arguments<'_>) {
    CONSOLE.line(args);
}

/// Queues one diagnostic line without allowing the bounded text queue to
/// erase it.
///
/// This is intentionally restricted to reporting outside measured hot paths:
/// awaiting text capacity inside radio, network or traffic service would make
/// USB progress part of their runtime contract.
pub async fn runtime_log_reliably(args: Arguments<'_>) {
    CONSOLE.line_reliably(args).await;
}

/// Publish the application-visible Wi-Fi owner state used for command
/// admission. Zero is reserved for the boot interval before a role exists.
pub fn set_wifi_role(role: WifiRole) {
    let encoded = match role {
        WifiRole::Idle => 1,
        WifiRole::Station => 2,
        WifiRole::Monitor => 3,
        WifiRole::AccessPoint => 4,
        WifiRole::StationAccessPoint => 5,
    };
    WIFI_ROLE_STATE.store(encoded, Ordering::Release);
}

/// Waits without polling until the host chooses the boot's unique radio owner.
pub async fn receive_pre_initialization_request() -> PreInitializationRequest {
    // The IEEE 802.15.4 diagnostic images are mutually exclusive, so at most
    // one diagnostic channel competes with startup.
    #[cfg(feature = "i-event-status-probe")]
    {
        match select(
            STARTUP_CONFIGURATIONS.receive(),
            IEEE802154_EVENT_STATUS_PROBES.receive(),
        )
        .await
        {
            Either::First(configuration) => PreInitializationRequest::Startup(configuration),
            Either::Second(probe) => PreInitializationRequest::Ieee802154EventStatus(probe),
        }
    }
    #[cfg(feature = "i-ed-event-probe")]
    {
        match select(
            STARTUP_CONFIGURATIONS.receive(),
            IEEE802154_ED_EVENT_PROBES.receive(),
        )
        .await
        {
            Either::First(configuration) => PreInitializationRequest::Startup(configuration),
            Either::Second(probe) => PreInitializationRequest::Ieee802154EdEvent(probe),
        }
    }
    #[cfg(feature = "i-route-probe")]
    {
        match select(
            STARTUP_CONFIGURATIONS.receive(),
            IEEE802154_ROUTE_PROBES.receive(),
        )
        .await
        {
            Either::First(configuration) => PreInitializationRequest::Startup(configuration),
            Either::Second(probe) => PreInitializationRequest::Ieee802154Route(probe),
        }
    }
    #[cfg(all(feature = "i-radio", not(feature = "i-thread")))]
    {
        match select3(
            STARTUP_CONFIGURATIONS.receive(),
            IEEE802154_AIR_CHECKS.receive(),
            IEEE802154_SESSION_STARTS.receive(),
        )
        .await
        {
            Either3::First(configuration) => PreInitializationRequest::Startup(configuration),
            Either3::Second(check) => PreInitializationRequest::Ieee802154AirCheck(check),
            Either3::Third(start) => PreInitializationRequest::Ieee802154Session(start),
        }
    }
    #[cfg(feature = "i-thread")]
    {
        match embassy_futures::select::select4(
            STARTUP_CONFIGURATIONS.receive(),
            IEEE802154_AIR_CHECKS.receive(),
            IEEE802154_SESSION_STARTS.receive(),
            IEEE802154_THREAD_STARTS.receive(),
        )
        .await
        {
            embassy_futures::select::Either4::First(configuration) => {
                PreInitializationRequest::Startup(configuration)
            }
            embassy_futures::select::Either4::Second(check) => {
                PreInitializationRequest::Ieee802154AirCheck(check)
            }
            embassy_futures::select::Either4::Third(start) => {
                PreInitializationRequest::Ieee802154Session(start)
            }
            embassy_futures::select::Either4::Fourth(start) => {
                PreInitializationRequest::Ieee802154Thread(start)
            }
        }
    }
    #[cfg(not(feature = "i-diagnostic"))]
    {
        PreInitializationRequest::Startup(STARTUP_CONFIGURATIONS.receive().await)
    }
}

/// Publish the role-neutral completion edge for one initialization command.
pub async fn complete_initialization(request_id: u32) {
    publish_event_reliably(0, request_id, oer_tool_data_hil_protocol::wifi::Initialized).await;
}

/// Returns the current target-defined startup artifact to the host in bounded
/// wire frames. This runs before traffic measurement and never writes flash or
/// NVS on the target.
pub async fn publish_startup_artifact(
    disposition: StartupArtifactDisposition,
    initialization_elapsed_micros: u64,
    bytes: &[u8],
) {
    let Ok(total_length) = u16::try_from(bytes.len()) else {
        CONSOLE.lose_message();
        return;
    };
    let checksum = startup_artifact_crc32c(bytes);
    publish_event_reliably(
        0,
        0,
        oer_tool_data_hil_protocol::phy::StartupArtifactReady(StartupArtifactStatus {
            disposition,
            total_length,
            initialization_elapsed_micros,
        }),
    )
    .await;
    for (index, part) in bytes.chunks(STARTUP_ARTIFACT_CHUNK_MAX_LEN).enumerate() {
        let offset = index * STARTUP_ARTIFACT_CHUNK_MAX_LEN;
        let Ok(offset) = u16::try_from(offset) else {
            CONSOLE.lose_message();
            return;
        };
        let Ok(chunk) = StartupArtifactChunk::try_new(total_length, offset, checksum, part) else {
            CONSOLE.lose_message();
            return;
        };
        publish_event_reliably(0, 0, oer_tool_data_hil_protocol::phy::StartupArtifactPart(chunk)).await;
    }
}

/// Waits until the host has configured, armed, and started one benchmark.
pub async fn receive_session_start() -> ActiveSession {
    SESSION_STARTS.receive().await
}

/// Wait until the host requests one finite connected-STA lifecycle cycle.
///
/// The production runner observes this only at a hardware-safe transaction
/// boundary; the console owns command admission and never touches radio state.
pub async fn receive_wifi_control_request() -> WifiControlRequest {
    WIFI_CONTROL_REQUESTS.receive().await
}

/// Reliably acknowledge a completed target-side station ownership cycle.
///
/// Unlike text diagnostics, this event is serialized by the protocol owner
/// and retains the command request ID used by the host qualifier.
pub async fn complete_station_epoch_cycle(request_id: u32, evidence: StationEpochEvidence) {
    publish_event_reliably(
        0,
        request_id,
        oer_tool_data_hil_protocol::wifi::StationEpochCompleted(evidence),
    )
    .await;
}

/// Reliably acknowledge complete station dematerialization and reconstruction
/// of the role-neutral Wi-Fi owner.
pub async fn complete_wifi_role_transition(request_id: u32, evidence: WifiRoleTransitionEvidence) {
    let sequence = queue_event_reliably(
        0,
        request_id,
        oer_tool_data_hil_protocol::wifi::RoleTransitioned(evidence),
    )
    .await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_wifi_radio_restart(
    request_id: u32,
    evidence: oer_tool_data_hil_protocol::wifi::WifiRadioRestartEvidence,
) {
    let sequence = queue_event_reliably(
        0,
        request_id,
        oer_tool_data_hil_protocol::wifi::RadioRestarted(evidence),
    )
    .await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_wifi_scan(request_id: u32, evidence: WifiScanEvidence) {
    let sequence = queue_event_reliably(
        0,
        request_id,
        oer_tool_data_hil_protocol::wifi::ScanCompleted(evidence),
    )
    .await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_monitor_start(request_id: u32, evidence: WifiRoleTransitionEvidence) {
    let sequence = queue_event_reliably(
        0,
        request_id,
        oer_tool_data_hil_protocol::wifi::MonitorStarted(evidence),
    )
    .await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_monitor_stop(request_id: u32, evidence: WifiMonitorEvidence) {
    let sequence = queue_event_reliably(
        0,
        request_id,
        oer_tool_data_hil_protocol::wifi::MonitorStopped(evidence),
    )
    .await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_access_point_start(request_id: u32, evidence: WifiRoleTransitionEvidence) {
    let sequence = queue_event_reliably(
        0,
        request_id,
        oer_tool_data_hil_protocol::wifi::AccessPointStarted(evidence),
    )
    .await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_access_point_stop(request_id: u32, evidence: WifiAccessPointEvidence) {
    let sequence = queue_event_reliably(
        0,
        request_id,
        oer_tool_data_hil_protocol::wifi::AccessPointStopped(evidence),
    )
    .await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_station_access_point_stop(
    request_id: u32,
    evidence: WifiStationAccessPointStopEvidence,
) {
    let sequence = queue_event_reliably(
        0,
        request_id,
        oer_tool_data_hil_protocol::wifi::StationAccessPointStopped(evidence),
    )
    .await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_wifi_role_failure(request_id: u32, evidence: WifiRoleFailureEvidence) {
    let sequence =
        queue_event_reliably(0, request_id, oer_tool_data_hil_protocol::wifi::RoleFailed(evidence)).await;
    wait_until_serialized(sequence).await;
}

pub async fn publish_monitor_frame(request_id: u32, chunk: WifiMonitorFrameChunk) {
    queue_event_reliably(0, request_id, oer_tool_data_hil_protocol::wifi::MonitorFrame(chunk)).await;
}

pub async fn complete_monitor_capture(request_id: u32, evidence: WifiMonitorEvidence) {
    let sequence = queue_event_reliably(
        0,
        request_id,
        oer_tool_data_hil_protocol::wifi::MonitorCaptureCompleted(evidence),
    )
    .await;
    wait_until_serialized(sequence).await;
}

async fn wait_until_serialized(sequence: u32) {
    CONSOLE.written(sequence).await;
}

/// Reliably publish one unsolicited station generation/link edge.
///
/// The caller emits this only after the corresponding ownership transition;
/// unlike UART text, it cannot be dropped under diagnostic pressure.
pub async fn publish_station_lifecycle(event: StationLifecycleEvent) {
    let sequence =
        queue_event_reliably(0, 0, oer_tool_data_hil_protocol::wifi::StationLifecycle(event)).await;
    wait_until_serialized(sequence).await;
}

/// Hands a completed in-memory measurement back to the protocol owner.
///
/// USB serialization happens in another task and therefore cannot extend the
/// benchmark's measured interval. The result is built before the returned
/// future, which holds it once: the arguments are not kept across the wait.
pub fn complete_session(
    session_id: u64,
    flow_evidence: [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY],
    radio: Option<oer_tool_data_hil_protocol::network::RadioEvidence>,
    tx_timing: Option<oer_tool_data_hil_protocol::network::TxAggregateTimingEvidence>,
    rx_delivery: Option<RxDeliveryEvidence>,
    rx_zero_copy: Option<oer_tool_data_hil_protocol::network::RxZeroCopyEvidence>,
    cpu_load: Option<oer_tool_data_hil_protocol::network::CpuLoadEvidence>,
    verdict: oer_tool_data_hil_protocol::network::SessionVerdict,
) -> impl core::future::Future<Output = ()> {
    let evidence = TransportEvidence::from_flows(flow_evidence);
    SESSION_RESULTS.send(SessionResult {
        session_id,
        flow_evidence,
        evidence,
        radio,
        tx_timing,
        rx_delivery,
        rx_zero_copy,
        cpu_load,
        verdict,
    })
}
