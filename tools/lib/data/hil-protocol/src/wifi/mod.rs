//! The Wi-Fi module: runtime initialization, role transitions, scans,
//! monitor capture and access-point reports, with the properties Wi-Fi
//! images advertise.

#[cfg(feature = "wifi")]
use postcard_schema::Schema;
#[cfg(feature = "wifi")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "wifi")]
mod configuration;
#[cfg(feature = "wifi")]
pub use configuration::*;
#[cfg(feature = "wifi")]
mod payload;
#[cfg(feature = "wifi")]
pub use payload::*;
#[cfg(feature = "wifi")]
mod airtime;
#[cfg(feature = "wifi")]
pub use airtime::*;
#[cfg(feature = "wifi")]
mod rx;
#[cfg(feature = "wifi")]
pub use rx::*;
#[cfg(feature = "wifi")]
mod tx;
#[cfg(feature = "wifi")]
pub use tx::*;

crate::messages! {
    /// Explicit role-neutral Wi-Fi lifecycle commands.
    property RoleControl = "wifi/role-control";
    /// Initialize calibration and the network stack without materializing a
    /// Wi-Fi role.
    property RuntimeInitialization = "wifi/runtime-initialization";
    /// Stop one healthy connected STA epoch at a safe runner boundary and
    /// use the returned owners to exercise reassociation.
    property StationEpochControl = "wifi/station-epoch-control";
    /// The bounded WPA2-Personal access point role.
    property AccessPoint = "wifi/access-point";
    /// One same-channel station plus access point owner, both network
    /// endpoints at the same time.
    property StationAccessPoint = "wifi/station-access-point";
    /// One finite normalized monitor capture exported as typed frame chunks.
    property MonitorCapture = "wifi/monitor-capture";
    /// Connected generations and proved peer-loss transitions, reported
    /// independently of lossy text diagnostics.
    property StationLifecycleEvents = "wifi/station-lifecycle-events";
    /// Driver-side value observers. Performance images leave the observation
    /// graph absent from the compiled datapath.
    property DriverObservation = "wifi/driver-observation";
    /// Invasive alternative TX backing and materialization implementations
    /// for same-ELF A/B experiments; never a production residence budget.
    property TxArchitectureProbe = "wifi/tx-architecture-probe";
    #[cfg(feature = "wifi")]
    endpoint Initialize = "wifi/initialize" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic Initialized = "wifi/initialized";
    #[cfg(feature = "wifi")]
    endpoint CycleStationEpoch = "wifi/station/epoch/cycle" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic StationEpochCompleted = "wifi/station/epoch/completed";
    #[cfg(feature = "wifi")]
    endpoint StopStation = "wifi/station/stop" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    endpoint StartStation = "wifi/station/start" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic StationLifecycle = "wifi/station/lifecycle";
    #[cfg(feature = "wifi")]
    endpoint Scan = "wifi/scan" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic ScanCompleted = "wifi/scan/completed";
    #[cfg(feature = "wifi")]
    endpoint StartMonitor = "wifi/monitor/start" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic MonitorStarted = "wifi/monitor/started";
    #[cfg(feature = "wifi")]
    endpoint StopMonitor = "wifi/monitor/stop" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic MonitorStopped = "wifi/monitor/stopped";
    #[cfg(feature = "wifi")]
    endpoint CaptureMonitor = "wifi/monitor/capture" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic MonitorFrame = "wifi/monitor/frame";
    #[cfg(feature = "wifi")]
    topic MonitorCaptureCompleted = "wifi/monitor/capture/completed";
    #[cfg(feature = "wifi")]
    endpoint StartAccessPoint = "wifi/access-point/start" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic AccessPointStarted = "wifi/access-point/started";
    #[cfg(feature = "wifi")]
    endpoint StopAccessPoint = "wifi/access-point/stop" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic AccessPointStopped = "wifi/access-point/stopped";
    #[cfg(feature = "wifi")]
    topic AccessPointAggregateFill = "wifi/access-point/aggregate-fill";
    #[cfg(feature = "wifi")]
    topic AirtimePeer = "wifi/access-point/airtime/peer";
    #[cfg(feature = "wifi")]
    topic AirtimeReport = "wifi/access-point/airtime/report";
    #[cfg(feature = "wifi")]
    endpoint StartStationAccessPoint = "wifi/station-access-point/start" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    endpoint StopStationAccessPoint = "wifi/station-access-point/stop" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic StationAccessPointStopped = "wifi/station-access-point/stopped";
    #[cfg(feature = "wifi")]
    topic RoleTransitioned = "wifi/role/transitioned";
    #[cfg(feature = "wifi")]
    topic RoleFailed = "wifi/role/failed";
    #[cfg(feature = "wifi")]
    endpoint RestartRadio = "wifi/radio/restart" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic RadioRestarted = "wifi/radio/restarted";
}

/// Initialize calibration and the network stack without materializing a
/// Wi-Fi role. This command is accepted exactly once per boot.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Initialize(pub crate::wifi::InitializationConfiguration);

/// The radio and shared Wi-Fi owner are ready in the role-neutral idle
/// state. The request ID correlates this edge with its request.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Initialized;

/// Request one connected STA teardown/reassociation cycle. This is a HIL
/// lifecycle operation, not a transport-session stop and not evidence of
/// peer link loss.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CycleStationEpoch;

/// Reliable completion acknowledgement for [`CycleStationEpoch`].
/// The envelope request ID identifies the command being completed.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationEpochCompleted(pub crate::wifi::StationEpochEvidence);

/// Stop the active station and return to role-neutral Wi-Fi ownership.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StopStation;

/// Materialize a station from the role-neutral Wi-Fi owner.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartStation(pub crate::wifi::StationStart);

/// Unsolicited, reliable station generation/link transition.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationLifecycle(pub crate::wifi::StationLifecycleEvent);

/// Run one finite standalone scan and return to role-neutral ownership.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Scan(pub crate::wifi::WifiScanRequest);

/// Reliable completion of one finite `ScanWifi` request.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ScanCompleted(pub crate::wifi::WifiScanEvidence);

/// Materialize a standalone monitor from the role-neutral Wi-Fi owner.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartMonitor(pub crate::wifi::WifiMonitorRequest);

/// Reliable completion of `StartMonitor`.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct MonitorStarted(pub crate::wifi::WifiRoleTransitionEvidence);

/// Stop the active monitor and return to role-neutral Wi-Fi ownership.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StopMonitor;

/// Reliable completion of `StopMonitor` and its bounded capture summary.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct MonitorStopped(pub crate::wifi::WifiMonitorEvidence);

/// Run one finite monitor epoch, export its captured frames, return to
/// idle and publish a terminal capture summary.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CaptureMonitor(pub crate::wifi::WifiMonitorCaptureRequest);

/// One ordered chunk emitted by `CaptureMonitor`.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct MonitorFrame(pub crate::wifi::WifiMonitorFrameChunk);

/// Terminal completion of one finite `CaptureMonitor` request.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct MonitorCaptureCompleted(pub crate::wifi::WifiMonitorEvidence);

/// Materialize one bounded WPA2-Personal access point from role-neutral
/// Wi-Fi ownership. Credentials belong to this AP epoch and are cleared
/// when the command value is dropped.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartAccessPoint(pub crate::wifi::WifiAccessPointRequest);

/// Reliable completion of `StartAccessPoint`.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AccessPointStarted(pub crate::wifi::WifiRoleTransitionEvidence);

/// Stop the active access point and return to role-neutral Wi-Fi ownership.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StopAccessPoint;

/// Reliable completion of `StopAccessPoint` and its bounded AP summary.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AccessPointStopped(pub crate::wifi::WifiAccessPointEvidence);

/// Per-association A-MPDU fill of one AP epoch, emitted before the
/// correlated stop.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AccessPointAggregateFill(pub crate::wifi::WifiApAggregateFill);

/// AP-epoch modelled service accounting, emitted before the correlated stop.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AirtimePeer(pub crate::wifi::WifiAirtimePeerEvidence);

/// Completeness of the preceding bounded peer records for this AP epoch.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AirtimeReport(pub crate::wifi::WifiAirtimeReport);

/// Materialize one same-channel upstream station plus downstream SoftAP.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartStationAccessPoint(pub crate::wifi::WifiStationAccessPointRequest);

/// Stop both paired roles and return every physical owner to idle.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StopStationAccessPoint;

/// Reliable completion of `StopStationAccessPoint` with the AP-side
/// timing report retained from the same physical epoch.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationAccessPointStopped(pub crate::wifi::WifiStationAccessPointStopEvidence);

/// Reliable completion of a Wi-Fi role transition or idle radio restart.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RoleTransitioned(pub crate::wifi::WifiRoleTransitionEvidence);

/// Terminal failure of a correlated role start/stop command.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RoleFailed(pub crate::wifi::WifiRoleFailureEvidence);

/// Take Wi-Fi off the shared radio and bring it up again from
/// role-neutral ownership.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RestartRadio;

/// Reliable completion of an idle Wi-Fi restart on the shared radio.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RadioRestarted(pub crate::wifi::WifiRadioRestartEvidence);
