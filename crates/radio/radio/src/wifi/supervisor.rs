//! Hardware-free application boundary for runtime radio supervision.
//!
//! Physical owners remain in one local supervisor actor. This module contains
//! only value reports and a controller over an implementation-provided port;
//! it deliberately does not model a detached owner-holding service.

use core::{fmt, future::Future};

use crate::wifi::{
    AccessPointRequest, MonitorRequest, StationAccessPointRequest, StationRequest, WifiScanRequest,
};

pub const WIFI_SCAN_RESULT_CAPACITY: usize = 32;

/// Application-facing subset of one BSS observation.
///
/// Association-specific IEs remain in the internal scan table. A later
/// station request performs its own fresh candidate scan instead of trusting
/// a stale application snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiScanResult {
    ssid: [u8; 32],
    ssid_len: u8,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi_dbm: i8,
    pub privacy: bool,
    pub rsn: bool,
    pub legacy_wpa: bool,
    pub ht: bool,
    pub he: bool,
}

impl WifiScanResult {
    pub const EMPTY: Self = Self {
        ssid: [0; 32],
        ssid_len: 0,
        bssid: [0; 6],
        channel: 0,
        rssi_dbm: i8::MIN,
        privacy: false,
        rsn: false,
        legacy_wpa: false,
        ht: false,
        he: false,
    };

    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        ssid: [u8; 32],
        ssid_len: u8,
        bssid: [u8; 6],
        channel: u8,
        rssi_dbm: i8,
        privacy: bool,
        rsn: bool,
        legacy_wpa: bool,
        ht: bool,
        he: bool,
    ) -> Self {
        Self {
            ssid,
            ssid_len: if ssid_len > 32 { 32 } else { ssid_len },
            bssid,
            channel,
            rssi_dbm,
            privacy,
            rsn,
            legacy_wpa,
            ht,
            he,
        }
    }

    pub fn ssid(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_len)]
    }
}

/// Complete bounded result of one finite standalone scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiScanReport {
    generation: RadioSubsystemGeneration,
    results: [WifiScanResult; WIFI_SCAN_RESULT_CAPACITY],
    result_count: u8,
    pub observed_frames: u32,
    pub dropped_unique_bss: u32,
}

impl WifiScanReport {
    pub const fn new(
        generation: RadioSubsystemGeneration,
        results: [WifiScanResult; WIFI_SCAN_RESULT_CAPACITY],
        result_count: u8,
        observed_frames: u32,
        dropped_unique_bss: u32,
    ) -> Self {
        Self {
            generation,
            results,
            result_count: if result_count as usize > WIFI_SCAN_RESULT_CAPACITY {
                WIFI_SCAN_RESULT_CAPACITY as u8
            } else {
                result_count
            },
            observed_frames,
            dropped_unique_bss,
        }
    }

    pub const fn generation(&self) -> RadioSubsystemGeneration {
        self.generation
    }

    pub fn results(&self) -> &[WifiScanResult] {
        &self.results[..usize::from(self.result_count)]
    }
}

/// Driver-side result of a finite scan request.
#[derive(Debug)]
pub enum WifiScanFailure<R, E> {
    Rejected { request: R, error: E },
    Returned { request: R, error: E },
    Faulted { error: E },
}

/// Application-side scan failure with role-neutral Wi-Fi returned whenever
/// the supervisor proved the complete owner graph reusable.
pub enum WifiScanOperationFailure<W, R, E> {
    Rejected { wifi: W, request: R, error: E },
    Failed { wifi: W, request: R, error: E },
    Faulted { error: E },
}

impl<W, R: fmt::Debug, E: fmt::Debug> fmt::Debug for WifiScanOperationFailure<W, R, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected { request, error, .. } => formatter
                .debug_struct("Rejected")
                .field("request", request)
                .field("error", error)
                .finish(),
            Self::Failed { request, error, .. } => formatter
                .debug_struct("Failed")
                .field("request", request)
                .field("error", error)
                .finish(),
            Self::Faulted { error } => formatter.debug_tuple("Faulted").field(error).finish(),
        }
    }
}

/// Wrapping identity of one successfully started Wi-Fi service epoch.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct RadioSubsystemGeneration(u32);

impl RadioSubsystemGeneration {
    pub const INITIAL: Self = Self(0);

    pub const fn value(self) -> u32 {
        self.0
    }

    /// Allocate the identity of the next successfully materialized epoch.
    ///
    /// Wrapping is intentional: this is a stale-completion discriminator, not
    /// a globally unique persistent identifier.
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

pub type WifiStartResult<R, E> = Result<WifiStartReport, WifiStartFailure<R, E>>;

/// Successful role start acknowledged by the owner-holding supervisor actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiStartReport {
    generation: RadioSubsystemGeneration,
}

impl WifiStartReport {
    pub const fn new(generation: RadioSubsystemGeneration) -> Self {
        Self { generation }
    }

    pub const fn generation(self) -> RadioSubsystemGeneration {
        self.generation
    }
}

/// Successful stop after the active role returned its DMA/ISR owner graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiStopReport {
    generation: RadioSubsystemGeneration,
}

/// What the shared radio did while Wi-Fi left and rejoined it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiRadioRestartRf {
    /// Wi-Fi was the last PHY client: RF was closed and woken again.
    ClosedAndWoken,
    /// Another PHY client kept RF open; Wi-Fi only left and rejoined.
    KeptOpen,
}

/// Successful restart of Wi-Fi on the shared radio while no Wi-Fi role was
/// active: Wi-Fi left the radio, and a fresh Wi-Fi bring-up rejoined it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiRadioRestartReport {
    generation: RadioSubsystemGeneration,
    rf: WifiRadioRestartRf,
}

impl WifiRadioRestartReport {
    pub const fn new(generation: RadioSubsystemGeneration, rf: WifiRadioRestartRf) -> Self {
        Self { generation, rf }
    }

    pub const fn generation(self) -> RadioSubsystemGeneration {
        self.generation
    }

    pub const fn rf(self) -> WifiRadioRestartRf {
        self.rf
    }
}

impl WifiStopReport {
    pub const fn new(generation: RadioSubsystemGeneration) -> Self {
        Self { generation }
    }

    pub const fn generation(self) -> RadioSubsystemGeneration {
        self.generation
    }
}

/// Start failure at the application control boundary.
///
/// A command rejected before hardware moves returns the untouched request. A
/// faulted result cannot return it because materialization may already retain
/// request-owned security state at a quarantined hardware frontier.
#[derive(Debug)]
pub enum WifiStartFailure<R, E> {
    Rejected { request: R, error: E },
    Faulted { error: E },
}

impl<R, E> WifiStartFailure<R, E> {
    pub const fn rejected(request: R, error: E) -> Self {
        Self::Rejected { request, error }
    }

    pub const fn faulted(error: E) -> Self {
        Self::Faulted { error }
    }
}

/// Hardware-free transport implemented by the physical supervisor mailbox.
///
/// Implementations publish requests to the single local actor which owns the
/// radio. They never execute ownership transitions in the application task.
pub trait WifiSupervisorPort {
    type Error;

    fn scan(
        &mut self,
        request: WifiScanRequest,
    ) -> impl Future<Output = Result<WifiScanReport, WifiScanFailure<WifiScanRequest, Self::Error>>> + '_;

    fn start_station(
        &mut self,
        request: StationRequest,
    ) -> impl Future<Output = WifiStartResult<StationRequest, Self::Error>> + '_;

    fn start_access_point(
        &mut self,
        request: AccessPointRequest,
    ) -> impl Future<Output = WifiStartResult<AccessPointRequest, Self::Error>> + '_;

    fn start_station_access_point(
        &mut self,
        request: StationAccessPointRequest,
    ) -> impl Future<Output = WifiStartResult<StationAccessPointRequest, Self::Error>> + '_;

    fn start_monitor(
        &mut self,
        request: MonitorRequest,
    ) -> impl Future<Output = WifiStartResult<MonitorRequest, Self::Error>> + '_;

    fn stop(&mut self) -> impl Future<Output = Result<WifiStopReport, Self::Error>> + '_;

    fn restart_radio(
        &mut self,
    ) -> impl Future<Output = Result<WifiRadioRestartReport, Self::Error>> + '_;
}

/// Role-neutral Wi-Fi control capability.
///
/// This is the only state from which a role can be started. Starting a role
/// consumes it, so station and monitor ownership cannot overlap in safe code.
pub struct WifiIdle<P> {
    port: P,
}

impl<P> WifiIdle<P> {
    pub const fn new(port: P) -> Self {
        Self { port }
    }

    pub fn into_port(self) -> P {
        self.port
    }
}

impl<P: WifiSupervisorPort> WifiIdle<P> {
    /// Take Wi-Fi off the shared radio and bring it up again while retaining
    /// this idle application control capability.
    pub async fn restart_radio(mut self) -> Result<(Self, WifiRadioRestartReport), P::Error> {
        let report = self.port.restart_radio().await?;
        Ok((self, report))
    }

    pub async fn scan(
        mut self,
        request: WifiScanRequest,
    ) -> Result<WifiScanCompleted<P>, WifiScanOperationFailure<Self, WifiScanRequest, P::Error>>
    {
        match self.port.scan(request).await {
            Ok(report) => Ok(WifiScanCompleted { wifi: self, report }),
            Err(WifiScanFailure::Rejected { request, error }) => {
                Err(WifiScanOperationFailure::Rejected {
                    wifi: self,
                    request,
                    error,
                })
            }
            Err(WifiScanFailure::Returned { request, error }) => {
                Err(WifiScanOperationFailure::Failed {
                    wifi: self,
                    request,
                    error,
                })
            }
            Err(WifiScanFailure::Faulted { error }) => {
                Err(WifiScanOperationFailure::Faulted { error })
            }
        }
    }

    pub async fn start_station(
        mut self,
        request: StationRequest,
    ) -> Result<WifiStation<P>, WifiRoleStartFailure<Self, StationRequest, P::Error>> {
        match self.port.start_station(request).await {
            Ok(report) => Ok(WifiStation {
                port: self.port,
                generation: report.generation(),
            }),
            Err(WifiStartFailure::Rejected { request, error }) => {
                Err(WifiRoleStartFailure::Rejected {
                    wifi: self,
                    request,
                    error,
                })
            }
            Err(WifiStartFailure::Faulted { error }) => {
                Err(WifiRoleStartFailure::Faulted { error })
            }
        }
    }

    pub async fn start_access_point(
        mut self,
        request: AccessPointRequest,
    ) -> Result<WifiAccessPoint<P>, WifiRoleStartFailure<Self, AccessPointRequest, P::Error>> {
        match self.port.start_access_point(request).await {
            Ok(report) => Ok(WifiAccessPoint {
                port: self.port,
                generation: report.generation(),
            }),
            Err(WifiStartFailure::Rejected { request, error }) => {
                Err(WifiRoleStartFailure::Rejected {
                    wifi: self,
                    request,
                    error,
                })
            }
            Err(WifiStartFailure::Faulted { error }) => {
                Err(WifiRoleStartFailure::Faulted { error })
            }
        }
    }

    pub async fn start_station_access_point(
        mut self,
        request: StationAccessPointRequest,
    ) -> Result<
        WifiStationAccessPoint<P>,
        WifiRoleStartFailure<Self, StationAccessPointRequest, P::Error>,
    > {
        match self.port.start_station_access_point(request).await {
            Ok(report) => Ok(WifiStationAccessPoint {
                port: self.port,
                generation: report.generation(),
            }),
            Err(WifiStartFailure::Rejected { request, error }) => {
                Err(WifiRoleStartFailure::Rejected {
                    wifi: self,
                    request,
                    error,
                })
            }
            Err(WifiStartFailure::Faulted { error }) => {
                Err(WifiRoleStartFailure::Faulted { error })
            }
        }
    }

    pub async fn start_monitor(
        mut self,
        request: MonitorRequest,
    ) -> Result<WifiMonitor<P>, WifiRoleStartFailure<Self, MonitorRequest, P::Error>> {
        match self.port.start_monitor(request).await {
            Ok(report) => Ok(WifiMonitor {
                port: self.port,
                generation: report.generation(),
            }),
            Err(WifiStartFailure::Rejected { request, error }) => {
                Err(WifiRoleStartFailure::Rejected {
                    wifi: self,
                    request,
                    error,
                })
            }
            Err(WifiStartFailure::Faulted { error }) => {
                Err(WifiRoleStartFailure::Faulted { error })
            }
        }
    }
}

/// Returned role-neutral owner and value-only observations from one scan.
pub struct WifiScanCompleted<P> {
    pub wifi: WifiIdle<P>,
    pub report: WifiScanReport,
}

impl<P> WifiScanCompleted<P> {
    pub fn into_parts(self) -> (WifiIdle<P>, WifiScanReport) {
        (self.wifi, self.report)
    }
}

/// Failed role start. A request rejected before hardware moved returns both
/// the request and the idle capability; a terminal hardware fault returns
/// neither because it is not legal to start another role.
pub enum WifiRoleStartFailure<W, R, E> {
    Rejected { wifi: W, request: R, error: E },
    Faulted { error: E },
}

impl<W, R: fmt::Debug, E: fmt::Debug> fmt::Debug for WifiRoleStartFailure<W, R, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected { request, error, .. } => formatter
                .debug_struct("Rejected")
                .field("request", request)
                .field("error", error)
                .finish(),
            Self::Faulted { error } => formatter
                .debug_struct("Faulted")
                .field("error", error)
                .finish(),
        }
    }
}

/// Active station role. It is neither `Clone` nor `Copy`; stopping consumes it
/// and returns [`WifiIdle`] only after the runner confirms quiescence.
pub struct WifiStation<P> {
    port: P,
    generation: RadioSubsystemGeneration,
}

/// Active access-point role. The handle contains control capability only;
/// the AP peer, key, beacon, DMA and interrupt owners remain in the physical
/// supervisor actor.
pub struct WifiAccessPoint<P> {
    port: P,
    generation: RadioSubsystemGeneration,
}

/// Active same-channel station plus SoftAP composition. The physical RX, TX
/// and IRQ owners remain unique inside the supervisor actor.
pub struct WifiStationAccessPoint<P> {
    port: P,
    generation: RadioSubsystemGeneration,
}

impl<P: WifiSupervisorPort> WifiStationAccessPoint<P> {
    pub const fn generation(&self) -> RadioSubsystemGeneration {
        self.generation
    }

    pub async fn stop(mut self) -> Result<WifiIdle<P>, WifiRoleStopFailure<P::Error>> {
        match self.port.stop().await {
            Ok(report) if report.generation() == self.generation => Ok(WifiIdle::new(self.port)),
            Ok(_) => Err(WifiRoleStopFailure::GenerationMismatch),
            Err(error) => Err(WifiRoleStopFailure::Faulted(error)),
        }
    }
}

impl<P: WifiSupervisorPort> WifiAccessPoint<P> {
    pub const fn generation(&self) -> RadioSubsystemGeneration {
        self.generation
    }

    pub async fn stop(mut self) -> Result<WifiIdle<P>, WifiRoleStopFailure<P::Error>> {
        match self.port.stop().await {
            Ok(report) if report.generation() == self.generation => Ok(WifiIdle::new(self.port)),
            Ok(_) => Err(WifiRoleStopFailure::GenerationMismatch),
            Err(error) => Err(WifiRoleStopFailure::Faulted(error)),
        }
    }
}

impl<P: WifiSupervisorPort> WifiStation<P> {
    pub const fn generation(&self) -> RadioSubsystemGeneration {
        self.generation
    }

    pub async fn stop(mut self) -> Result<WifiIdle<P>, WifiRoleStopFailure<P::Error>> {
        match self.port.stop().await {
            Ok(report) if report.generation() == self.generation => Ok(WifiIdle::new(self.port)),
            Ok(_) => Err(WifiRoleStopFailure::GenerationMismatch),
            Err(error) => Err(WifiRoleStopFailure::Faulted(error)),
        }
    }
}

/// Active standalone monitor role.
pub struct WifiMonitor<P> {
    port: P,
    generation: RadioSubsystemGeneration,
}

impl<P: WifiSupervisorPort> WifiMonitor<P> {
    pub const fn generation(&self) -> RadioSubsystemGeneration {
        self.generation
    }

    pub async fn stop(mut self) -> Result<WifiIdle<P>, WifiRoleStopFailure<P::Error>> {
        match self.port.stop().await {
            Ok(report) if report.generation() == self.generation => Ok(WifiIdle::new(self.port)),
            Ok(_) => Err(WifiRoleStopFailure::GenerationMismatch),
            Err(error) => Err(WifiRoleStopFailure::Faulted(error)),
        }
    }
}

/// A role can return to idle only through a matching quiesced completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiRoleStopFailure<E> {
    GenerationMismatch,
    Faulted(E),
}

/// Application radio handle containing control capability only.
///
/// Physical RF, protocol owners, PAC, DMA and interrupt epochs remain in the
/// supervisor actor and cannot be extracted through this value.
pub struct RadioController<W> {
    wifi: WifiIdle<W>,
}

impl<W> RadioController<W> {
    pub const fn new(wifi: WifiIdle<W>) -> Self {
        Self { wifi }
    }

    /// Materialize the sole Wi-Fi control capability. The radio controller is
    /// consumed so a second Wi-Fi root cannot be produced.
    pub fn into_wifi(self) -> WifiIdle<W> {
        self.wifi
    }
}
