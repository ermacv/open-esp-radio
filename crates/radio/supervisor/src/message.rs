//! Bounded command, completion and transport-error values.

use oer_radio::wifi::{
    AccessPointRequest, MonitorRequest, StationAccessPointRequest, StationRequest,
    WifiRadioRestartReport, WifiScanFailure, WifiScanReport, WifiScanRequest, WifiStartResult,
    WifiStopReport,
};

/// Request transported to the sole owner-holding radio-supervisor task.
pub enum WifiSupervisorCommand {
    Scan(WifiScanRequest),
    StartStation(StationRequest),
    StartAccessPoint(AccessPointRequest),
    StartStationAccessPoint(StationAccessPointRequest),
    StartMonitor(MonitorRequest),
    Stop,
    RestartRadio,
}

/// Role requested while another Wi-Fi role graph is already active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiStartKind {
    StandaloneScan,
    Station,
    AccessPoint,
    StationAccessPoint,
    StandaloneMonitor,
    WholeRadioRestart,
}

/// Typed completion transported back to the application controller.
#[expect(
    clippy::large_enum_variant,
    reason = "the statically allocated single-response mailbox retains the complete bounded scan report"
)]
pub enum WifiSupervisorResponse<E> {
    Scan(Result<WifiScanReport, WifiScanFailure<WifiScanRequest, E>>),
    Station(WifiStartResult<StationRequest, E>),
    AccessPoint(WifiStartResult<AccessPointRequest, E>),
    StationAccessPoint(WifiStartResult<StationAccessPointRequest, E>),
    Monitor(WifiStartResult<MonitorRequest, E>),
    Stop(Result<WifiStopReport, E>),
    RestartRadio(Result<WifiRadioRestartReport, E>),
    SupervisorUnavailable,
}

/// Control transport failure distinct from a backend service error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiSupervisorError<E> {
    SupervisorUnavailable,
    ResponseMismatch,
    Service(E),
}
