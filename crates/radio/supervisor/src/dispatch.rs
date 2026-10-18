//! Owner-independent stopped-state planning and typed rejection.

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_radio::wifi::{
    WifiScanFailure, WifiServicePlanningError, WifiServiceRequest, WifiStartFailure,
    WifiStopReport, WifiSupervisorConfiguration,
};

use super::message::{WifiSupervisorCommand, WifiSupervisorResponse};
use super::transport::WifiSupervisorEndpoint;

/// Result of one command received while the supervisor holds `WifiStopped`.
///
/// `Handled` means an idempotent stop or a rejected start was already answered
/// without moving hardware. `Start` carries a fully capability-checked,
/// owner-independent service request which the concrete actor may now
/// materialize by consuming its stopped owner.
#[allow(
    clippy::large_enum_variant,
    reason = "a no-alloc dispatch hands the complete request with its credentials by value"
)]
pub enum WifiStoppedDispatch {
    Handled,
    Start(WifiServiceRequest),
    RestartRadio,
}

/// Validate and dispatch one command while the concrete actor owns a reusable
/// stopped Wi-Fi frontier.
///
/// Planning happens before any PAC/DMA/IRQ owner moves. Rejected starts return
/// their exact request, and an already-stopped `Stop` is acknowledged
/// idempotently with the current generation.
pub async fn dispatch_wifi_stopped_command<M, E, PlanningError>(
    endpoint: &mut WifiSupervisorEndpoint<'_, M, E>,
    configuration: WifiSupervisorConfiguration,
    generation: oer_radio::wifi::RadioSubsystemGeneration,
    mut planning_error: PlanningError,
) -> WifiStoppedDispatch
where
    M: RawMutex,
    PlanningError: FnMut(WifiServicePlanningError) -> E,
{
    match endpoint.receive().await {
        WifiSupervisorCommand::Scan(request) => match configuration.plan_scan(request) {
            Ok(service) => WifiStoppedDispatch::Start(service),
            Err(failure) => {
                endpoint
                    .respond(WifiSupervisorResponse::Scan(Err(
                        WifiScanFailure::Rejected {
                            request: failure.request,
                            error: planning_error(failure.error),
                        },
                    )))
                    .await;
                WifiStoppedDispatch::Handled
            }
        },
        WifiSupervisorCommand::StartStation(request) => match configuration.plan_station(request) {
            Ok(service) => WifiStoppedDispatch::Start(service),
            Err(failure) => {
                endpoint
                    .respond(WifiSupervisorResponse::Station(Err(
                        WifiStartFailure::rejected(failure.request, planning_error(failure.error)),
                    )))
                    .await;
                WifiStoppedDispatch::Handled
            }
        },
        WifiSupervisorCommand::StartAccessPoint(request) => {
            match configuration.plan_access_point(request) {
                Ok(service) => WifiStoppedDispatch::Start(service),
                Err(failure) => {
                    endpoint
                        .respond(WifiSupervisorResponse::AccessPoint(Err(
                            WifiStartFailure::rejected(
                                failure.request,
                                planning_error(failure.error),
                            ),
                        )))
                        .await;
                    WifiStoppedDispatch::Handled
                }
            }
        }
        WifiSupervisorCommand::StartStationAccessPoint(request) => {
            match configuration.plan_station_access_point(request) {
                Ok(service) => WifiStoppedDispatch::Start(service),
                Err(failure) => {
                    endpoint
                        .respond(WifiSupervisorResponse::StationAccessPoint(Err(
                            WifiStartFailure::rejected(
                                failure.request,
                                planning_error(failure.error),
                            ),
                        )))
                        .await;
                    WifiStoppedDispatch::Handled
                }
            }
        }
        WifiSupervisorCommand::StartMonitor(request) => match configuration.plan_monitor(request) {
            Ok(service) => WifiStoppedDispatch::Start(service),
            Err(failure) => {
                endpoint
                    .respond(WifiSupervisorResponse::Monitor(Err(
                        WifiStartFailure::rejected(failure.request, planning_error(failure.error)),
                    )))
                    .await;
                WifiStoppedDispatch::Handled
            }
        },
        WifiSupervisorCommand::Stop => {
            endpoint
                .respond(WifiSupervisorResponse::Stop(Ok(WifiStopReport::new(
                    generation,
                ))))
                .await;
            WifiStoppedDispatch::Handled
        }
        WifiSupervisorCommand::RestartRadio => WifiStoppedDispatch::RestartRadio,
    }
}
