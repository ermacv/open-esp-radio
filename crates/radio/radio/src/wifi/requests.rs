//! Application policy supplied when starting one Wi-Fi service.
//!
//! These values contain no peripheral, DMA or executor ownership.  The
//! topology and VIF identities remain in [`crate::wifi::WifiPlan`]; a request only
//! describes how an already validated role should operate for one runtime
//! epoch.

use core::{
    fmt,
    num::{NonZeroU8, NonZeroU16},
};

pub use oer_ieee80211_ap::{
    AccessPointClientLimit, AccessPointClientLimitError, AccessPointInactiveTimeout,
    AccessPointInactiveTimeoutError,
};
use oer_ieee80211_mac::{
    channel::WifiChannel,
    security::{LinkProtection, StaSecurityPolicy},
    ssid::WifiSsid,
};
use oer_ieee80211_rsn::{Pmk, PskDerivationError, sae::SaePassword};
use oer_ieee80211_softmac::{
    ESP_NOW_DEFAULT_PEER_CAPACITY, EspNowConfig, EspNowConfigError, EspNowPeerConfig, EspNowPeerId,
    EspNowPeerTableError, EspNowPhyMode, EspNowProtocol, MacServiceCapabilities,
    MonitorChannelPolicy, MonitorChannelSequence, WifiAccessPointConfig, WifiConfig,
    WifiConfigError, WifiStandaloneEspNowPlan, WifiStationConfig,
};
pub use oer_ieee80211_sta::request::{
    StationDiscovery, StationListenInterval, StationPowerMode, StationScanChannelIter,
    StationScanChannelOrderIter, StationScanChannels, StationScanChannelsError, StationScanPolicy,
};
use oer_ieee80211_sta::station::StaReconnectPolicy;

use crate::wifi::{WifiMonitorConfig, WifiPlan};

/// Complete plaintext request for one standalone ESP-NOW radio epoch.
///
/// The embedded protocol owner is already bound to the exclusive station VIF
/// and fixed home channel. Peers may be registered before the request is
/// moved into the chip runtime. A peer may explicitly select one fixed
/// standalone off-channel through its typed peer policy; there is no scan or
/// fallback. There is intentionally no credential, association, network,
/// encryption or LR field.
pub struct StandaloneEspNowRequest<const PEERS: usize = ESP_NOW_DEFAULT_PEER_CAPACITY> {
    plan: WifiStandaloneEspNowPlan,
    protocol: EspNowProtocol<PEERS>,
}

impl<const PEERS: usize> StandaloneEspNowRequest<PEERS> {
    pub fn new(
        plan: WifiStandaloneEspNowPlan,
        home_channel: WifiChannel,
    ) -> Result<Self, EspNowConfigError> {
        let protocol = EspNowProtocol::new(EspNowConfig::new(plan.station(), home_channel)?);
        Ok(Self { plan, protocol })
    }

    pub const fn plan(&self) -> WifiStandaloneEspNowPlan {
        self.plan
    }

    pub const fn home_channel(&self) -> WifiChannel {
        self.protocol.config().home_channel()
    }

    pub const fn protocol(&self) -> &EspNowProtocol<PEERS> {
        &self.protocol
    }

    pub fn add_peer(
        &mut self,
        peer: EspNowPeerConfig,
    ) -> Result<EspNowPeerId, StandaloneEspNowPeerError> {
        if matches!(peer.phy_mode(), EspNowPhyMode::LongRange(_)) {
            return Err(StandaloneEspNowPeerError::LongRangeUnsupported);
        }
        self.protocol
            .add_peer(peer)
            .map_err(StandaloneEspNowPeerError::PeerTable)
    }

    pub fn remove_peer(
        &mut self,
        peer: EspNowPeerId,
    ) -> Result<EspNowPeerConfig, EspNowPeerTableError> {
        self.protocol.remove_peer(peer)
    }

    pub fn into_parts(self) -> (WifiStandaloneEspNowPlan, EspNowProtocol<PEERS>) {
        (self.plan, self.protocol)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StandaloneEspNowPeerError {
    LongRangeUnsupported,
    PeerTable(EspNowPeerTableError),
}

impl fmt::Display for StandaloneEspNowPeerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LongRangeUnsupported => formatter
                .write_str("standalone ESP-NOW does not support the unqualified long-range PHY"),
            Self::PeerTable(error) => write!(formatter, "ESP-NOW peer table error: {error}"),
        }
    }
}

impl core::error::Error for StandaloneEspNowPeerError {}

impl<const PEERS: usize> fmt::Debug for StandaloneEspNowRequest<PEERS> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StandaloneEspNowRequest")
            .field("plan", &self.plan)
            .field("home_channel", &self.home_channel())
            .field("peer_count", &self.protocol.peers().len())
            .field("security", &"plaintext")
            .finish()
    }
}

/// Finite standalone scan policy.
///
/// The current backend sends a broadcast Probe Request on every selected
/// channel and continues receiving for the complete dwell. It neither joins a
/// BSS nor materializes the station role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiScanRequest {
    channels: StationScanChannels,
    dwell_millis: NonZeroU16,
}

impl WifiScanRequest {
    pub const fn new(channels: StationScanChannels, dwell_millis: NonZeroU16) -> Self {
        Self {
            channels,
            dwell_millis,
        }
    }

    pub const fn channels(self) -> StationScanChannels {
        self.channels
    }

    pub const fn dwell_millis(self) -> u16 {
        self.dwell_millis.get()
    }
}

/// Security material owned by one station runtime request.
///
/// Open owns no key material. WPA2-Personal owns the derived PMK and, since
/// the station upgrades to SAE whenever the access point offers it, the
/// password as well; WPA3-Personal owns only the password, from which every
/// attempt derives its SAE password element. [`Pmk`] and [`SaePassword`]
/// clear their bytes on drop and cannot be formatted.
pub enum StationSecurity {
    Open,
    Wpa2Personal { pmk: Pmk, password: SaePassword },
    Wpa3Personal(SaePassword),
}

/// Why personal station credentials were refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationCredentialError {
    Psk(PskDerivationError),
    /// An SAE password of 1 to 63 octets.
    SaePasswordLength,
}

impl StationSecurity {
    pub const fn open() -> Self {
        Self::Open
    }

    /// WPA2-Personal from a passphrase: PSK with the PMK derived from the
    /// passphrase and SSID, or SAE with the passphrase whenever the access
    /// point offers it.
    pub fn wpa2_personal(passphrase: &[u8], ssid: &[u8]) -> Result<Self, StationCredentialError> {
        let password =
            SaePassword::new(passphrase).ok_or(StationCredentialError::SaePasswordLength)?;
        let pmk = Pmk::derive(passphrase, ssid).map_err(StationCredentialError::Psk)?;
        Ok(Self::Wpa2Personal { pmk, password })
    }

    /// WPA3-Personal: SAE only.
    pub fn wpa3_personal(password: &[u8]) -> Result<Self, StationCredentialError> {
        SaePassword::new(password)
            .map(Self::Wpa3Personal)
            .ok_or(StationCredentialError::SaePasswordLength)
    }

    pub const fn policy(&self) -> StaSecurityPolicy {
        match self {
            Self::Open => StaSecurityPolicy::Open,
            Self::Wpa2Personal { .. } => StaSecurityPolicy::Wpa2Personal,
            Self::Wpa3Personal(_) => StaSecurityPolicy::Wpa3Personal,
        }
    }
}

impl fmt::Debug for StationSecurity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open => formatter.write_str("Open"),
            Self::Wpa2Personal { .. } => formatter.write_str("Wpa2Personal(<redacted>)"),
            Self::Wpa3Personal(_) => formatter.write_str("Wpa3Personal(<redacted>)"),
        }
    }
}

/// Security material owned by one access-point epoch.
///
/// The public boundary accepts a PMK rather than retaining a plaintext
/// passphrase. Nonces, GTK material and replay counters are generated by the
/// AP owner when the epoch starts.
pub enum AccessPointSecurity {
    Open,
    Wpa2Personal(Pmk),
}

impl AccessPointSecurity {
    pub const fn open() -> Self {
        Self::Open
    }

    pub const fn wpa2_personal(pmk: Pmk) -> Self {
        Self::Wpa2Personal(pmk)
    }

    pub const fn mode(&self) -> LinkProtection {
        match self {
            Self::Open => LinkProtection::Open,
            Self::Wpa2Personal(_) => LinkProtection::Ccmp,
        }
    }

    pub const fn pmk(&self) -> Option<&Pmk> {
        match self {
            Self::Open => None,
            Self::Wpa2Personal(pmk) => Some(pmk),
        }
    }

    pub fn into_pmk(self) -> Option<Pmk> {
        match self {
            Self::Open => None,
            Self::Wpa2Personal(pmk) => Some(pmk),
        }
    }
}

impl fmt::Debug for AccessPointSecurity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open => formatter.write_str("Open"),
            Self::Wpa2Personal(_) => formatter.write_str("Wpa2Personal(<redacted>)"),
        }
    }
}

/// Nonzero IEEE beacon interval in timing units (TU).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointBeaconInterval(NonZeroU16);

impl AccessPointBeaconInterval {
    pub const DEFAULT_TU: u16 = 100;

    pub const fn new(tu: u16) -> Result<Self, AccessPointBeaconIntervalError> {
        match NonZeroU16::new(tu) {
            Some(tu) => Ok(Self(tu)),
            None => Err(AccessPointBeaconIntervalError::Zero),
        }
    }

    pub const fn tu(self) -> u16 {
        self.0.get()
    }
}

impl Default for AccessPointBeaconInterval {
    fn default() -> Self {
        Self(NonZeroU16::new(Self::DEFAULT_TU).expect("default beacon interval is nonzero"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointBeaconIntervalError {
    Zero,
}

impl fmt::Display for AccessPointBeaconIntervalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("access-point beacon interval must be nonzero")
    }
}

impl core::error::Error for AccessPointBeaconIntervalError {}

/// Nonzero number of beacon intervals in one DTIM period.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointDtimPeriod(NonZeroU8);

impl AccessPointDtimPeriod {
    pub const DEFAULT: u8 = 2;

    pub const fn new(period: u8) -> Result<Self, AccessPointDtimPeriodError> {
        match NonZeroU8::new(period) {
            Some(period) => Ok(Self(period)),
            None => Err(AccessPointDtimPeriodError::Zero),
        }
    }

    pub const fn get(self) -> u8 {
        self.0.get()
    }
}

impl Default for AccessPointDtimPeriod {
    fn default() -> Self {
        Self(NonZeroU8::new(Self::DEFAULT).expect("default DTIM period is nonzero"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointDtimPeriodError {
    Zero,
}

impl fmt::Display for AccessPointDtimPeriodError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("access-point DTIM period must be nonzero")
    }
}

impl core::error::Error for AccessPointDtimPeriodError {}

/// Complete owned policy for one AP service epoch.
///
/// Beacon cadence, DTIM and peer capacity are validated before hardware
/// ownership moves. Defaults remain 100 TU and DTIM period 2; both cadence
/// values are explicit nonzero types so the TIM/DTIM publisher and buffering
/// policy consume the same request geometry.
pub struct AccessPointRequest {
    ssid: WifiSsid,
    security: AccessPointSecurity,
    channel: WifiChannel,
    client_limit: AccessPointClientLimit,
    inactive_timeout: AccessPointInactiveTimeout,
    beacon_interval: AccessPointBeaconInterval,
    dtim_period: AccessPointDtimPeriod,
}

impl AccessPointRequest {
    pub const BEACON_INTERVAL_TU: u16 = AccessPointBeaconInterval::DEFAULT_TU;
    pub const DTIM_PERIOD: u8 = AccessPointDtimPeriod::DEFAULT;
    pub const PEER_CAPACITY: usize = AccessPointClientLimit::MAX as usize;

    pub fn new(
        ssid: WifiSsid,
        security: AccessPointSecurity,
        channel: WifiChannel,
        client_limit: AccessPointClientLimit,
    ) -> Result<Self, AccessPointRequestError> {
        if channel.primary() == 14 {
            return Err(AccessPointRequestError::UnsupportedPrimaryChannel(14));
        }
        Ok(Self {
            ssid,
            security,
            channel,
            client_limit,
            inactive_timeout: AccessPointInactiveTimeout::default(),
            beacon_interval: AccessPointBeaconInterval(
                NonZeroU16::new(Self::BEACON_INTERVAL_TU)
                    .expect("default beacon interval is nonzero"),
            ),
            dtim_period: AccessPointDtimPeriod(
                NonZeroU8::new(Self::DTIM_PERIOD).expect("default DTIM period is nonzero"),
            ),
        })
    }

    pub const fn ssid(&self) -> &WifiSsid {
        &self.ssid
    }

    pub const fn security(&self) -> &AccessPointSecurity {
        &self.security
    }

    pub const fn channel(&self) -> WifiChannel {
        self.channel
    }

    pub const fn client_limit(&self) -> AccessPointClientLimit {
        self.client_limit
    }

    pub const fn inactive_timeout(&self) -> AccessPointInactiveTimeout {
        self.inactive_timeout
    }

    pub const fn beacon_interval(&self) -> AccessPointBeaconInterval {
        self.beacon_interval
    }

    pub const fn dtim_period(&self) -> AccessPointDtimPeriod {
        self.dtim_period
    }

    pub const fn with_inactive_timeout(mut self, timeout: AccessPointInactiveTimeout) -> Self {
        self.inactive_timeout = timeout;
        self
    }

    pub const fn with_beacon_cadence(
        mut self,
        beacon_interval: AccessPointBeaconInterval,
        dtim_period: AccessPointDtimPeriod,
    ) -> Self {
        self.beacon_interval = beacon_interval;
        self.dtim_period = dtim_period;
        self
    }

    pub fn into_parts(
        self,
    ) -> (
        WifiSsid,
        AccessPointSecurity,
        WifiChannel,
        AccessPointClientLimit,
        AccessPointInactiveTimeout,
        AccessPointBeaconInterval,
        AccessPointDtimPeriod,
    ) {
        (
            self.ssid,
            self.security,
            self.channel,
            self.client_limit,
            self.inactive_timeout,
            self.beacon_interval,
            self.dtim_period,
        )
    }
}

impl fmt::Debug for AccessPointRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccessPointRequest")
            .field("ssid", &self.ssid)
            .field("security", &self.security)
            .field("channel", &self.channel)
            .field("client_limit", &self.client_limit)
            .field("inactive_timeout", &self.inactive_timeout)
            .field("beacon_interval", &self.beacon_interval)
            .field("dtim_period", &self.dtim_period)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointRequestError {
    UnsupportedPrimaryChannel(u8),
}

impl fmt::Display for AccessPointRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPrimaryChannel(channel) => write!(
                formatter,
                "the current access-point service does not support primary channel {channel}"
            ),
        }
    }
}

impl core::error::Error for AccessPointRequestError {}

/// Complete owned request for one station service epoch.
pub struct StationRequest {
    ssid: WifiSsid,
    security: StationSecurity,
    reconnect: StaReconnectPolicy,
    scan: StationScanPolicy,
    power_mode: StationPowerMode,
}

impl StationRequest {
    pub const fn new(
        ssid: WifiSsid,
        security: StationSecurity,
        reconnect: StaReconnectPolicy,
        scan: StationScanPolicy,
    ) -> Self {
        Self {
            ssid,
            security,
            reconnect,
            scan,
            power_mode: StationPowerMode::None,
        }
    }

    pub const fn ssid(&self) -> &WifiSsid {
        &self.ssid
    }

    pub const fn security(&self) -> &StationSecurity {
        &self.security
    }

    pub const fn reconnect_policy(&self) -> StaReconnectPolicy {
        self.reconnect
    }

    pub const fn scan_policy(&self) -> StationScanPolicy {
        self.scan
    }

    pub const fn power_mode(&self) -> StationPowerMode {
        self.power_mode
    }

    pub const fn with_power_mode(mut self, power_mode: StationPowerMode) -> Self {
        self.power_mode = power_mode;
        self
    }

    pub fn into_parts(
        self,
    ) -> (
        StationDiscovery,
        StationSecurity,
        StaReconnectPolicy,
        StationPowerMode,
    ) {
        (
            StationDiscovery::new(self.ssid, self.scan),
            self.security,
            self.reconnect,
            self.power_mode,
        )
    }
}

impl fmt::Debug for StationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StationRequest")
            .field("ssid", &self.ssid)
            .field("security", &self.security)
            .field("reconnect", &self.reconnect)
            .field("scan", &self.scan)
            .field("power_mode", &self.power_mode)
            .finish()
    }
}

/// One same-channel station plus SoftAP service request.
///
/// The station is connected first. Its associated channel becomes the sole
/// physical channel context; the AP is started only when its requested
/// channel is exactly that channel. No multi-channel scheduling fallback is
/// permitted.
pub struct StationAccessPointRequest {
    station: StationRequest,
    access_point: AccessPointRequest,
}

impl StationAccessPointRequest {
    pub const fn new(station: StationRequest, access_point: AccessPointRequest) -> Self {
        Self {
            station,
            access_point,
        }
    }

    pub const fn station(&self) -> &StationRequest {
        &self.station
    }

    pub const fn access_point(&self) -> &AccessPointRequest {
        &self.access_point
    }

    pub const fn required_channel(&self) -> WifiChannel {
        self.access_point.channel()
    }

    pub fn into_parts(self) -> (StationRequest, AccessPointRequest) {
        (self.station, self.access_point)
    }
}

impl fmt::Debug for StationAccessPointRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StationAccessPointRequest")
            .field("station", &self.station)
            .field("access_point", &self.access_point)
            .field("channel_policy", &"station-associated-channel")
            .finish()
    }
}

/// Host/export capture policy independent of the DMA ring size.
///
/// Queue capacity and DMA placement belong to `RadioResources<Profile>`.
/// Saturation is always best-effort drop with accounting; no request may turn
/// a slow capture consumer into RX-DMA backpressure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorCapturePolicy {
    snapshot_length: Option<NonZeroU16>,
}

impl MonitorCapturePolicy {
    pub const fn complete_frames() -> Self {
        Self {
            snapshot_length: None,
        }
    }

    pub const fn truncate_at(snapshot_length: NonZeroU16) -> Self {
        Self {
            snapshot_length: Some(snapshot_length),
        }
    }

    pub const fn snapshot_length(self) -> Option<u16> {
        match self.snapshot_length {
            Some(length) => Some(length.get()),
            None => None,
        }
    }
}

impl Default for MonitorCapturePolicy {
    fn default() -> Self {
        Self::complete_frames()
    }
}

/// Complete value-only request for one standalone monitor epoch.
pub struct MonitorRequest {
    channels: MonitorChannelPolicy,
    monitor: WifiMonitorConfig,
    capture: MonitorCapturePolicy,
}

impl MonitorRequest {
    pub const fn new(channel: WifiChannel, monitor: WifiMonitorConfig) -> Self {
        Self {
            channels: MonitorChannelPolicy::fixed(channel),
            monitor,
            capture: MonitorCapturePolicy::complete_frames(),
        }
    }

    /// Capture repeatedly across one checked, ordered channel cycle.
    pub const fn hopping(sequence: MonitorChannelSequence, monitor: WifiMonitorConfig) -> Self {
        Self {
            channels: MonitorChannelPolicy::hopping(sequence),
            monitor,
            capture: MonitorCapturePolicy::complete_frames(),
        }
    }

    pub const fn with_capture_policy(mut self, capture: MonitorCapturePolicy) -> Self {
        self.capture = capture;
        self
    }

    pub const fn channel(&self) -> WifiChannel {
        self.channels.initial_channel()
    }

    pub const fn channel_policy(&self) -> MonitorChannelPolicy {
        self.channels
    }

    pub const fn monitor_config(&self) -> WifiMonitorConfig {
        self.monitor
    }

    pub const fn capture_policy(&self) -> MonitorCapturePolicy {
        self.capture
    }
}

impl fmt::Debug for MonitorRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MonitorRequest")
            .field("channels", &self.channels)
            .field("monitor", &self.monitor)
            .field("capture", &self.capture)
            .finish()
    }
}

/// One capability-checked Wi-Fi owner graph requested from the supervisor.
///
/// Every implemented topology has its own explicit variant and lifecycle
/// contract. Station, access point, same-channel STA+AP, finite scan and
/// standalone monitor therefore remain physically exclusive without encoding
/// owner graphs as combinations of optional fields.
pub enum WifiServiceRequest {
    StandaloneScan {
        plan: WifiPlan,
        request: WifiScanRequest,
    },
    Station {
        plan: WifiPlan,
        request: StationRequest,
    },
    AccessPoint {
        plan: WifiPlan,
        request: AccessPointRequest,
    },
    StationAccessPoint {
        plan: WifiPlan,
        request: StationAccessPointRequest,
    },
    StandaloneMonitor {
        plan: WifiPlan,
        request: MonitorRequest,
    },
}

impl WifiServiceRequest {
    /// Join a finite scan request to the station-capable hardware topology.
    pub fn standalone_scan(
        plan: WifiPlan,
        request: WifiScanRequest,
    ) -> Result<Self, WifiServiceRequestFailure<WifiScanRequest>> {
        if plan.station().is_none() {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::MissingScanTopology,
            ));
        }
        if plan.access_point().is_some() || plan.monitor().is_some() {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::UnexpectedTopologyRole,
            ));
        }
        Ok(Self::StandaloneScan { plan, request })
    }

    /// Join a checked station-only topology to its runtime policy.
    #[allow(
        clippy::result_large_err,
        reason = "a rejected no-alloc station request must return its exact credential owner"
    )]
    pub fn station(
        plan: WifiPlan,
        request: StationRequest,
    ) -> Result<Self, WifiServiceRequestFailure<StationRequest>> {
        if plan.station().is_none() {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::MissingStationTopology,
            ));
        }
        if plan.access_point().is_some() || plan.monitor().is_some() {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::UnexpectedTopologyRole,
            ));
        }
        Ok(Self::Station { plan, request })
    }

    /// Join a checked access-point-only topology to its runtime policy.
    pub fn access_point(
        plan: WifiPlan,
        request: AccessPointRequest,
    ) -> Result<Self, WifiServiceRequestFailure<AccessPointRequest>> {
        if plan.access_point().is_none() {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::MissingAccessPointTopology,
            ));
        }
        if plan.station().is_some() || plan.monitor().is_some() {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::UnexpectedTopologyRole,
            ));
        }
        Ok(Self::AccessPoint { plan, request })
    }

    /// Join a checked same-channel STA+AP topology to one combined request.
    #[allow(
        clippy::result_large_err,
        reason = "a rejected no-alloc combined request must return both exact credential owners"
    )]
    pub fn station_access_point(
        plan: WifiPlan,
        request: StationAccessPointRequest,
    ) -> Result<Self, WifiServiceRequestFailure<StationAccessPointRequest>> {
        if plan.station().is_none() {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::MissingStationTopology,
            ));
        }
        if plan.access_point().is_none() {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::MissingAccessPointTopology,
            ));
        }
        if plan.monitor().is_some() {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::UnexpectedTopologyRole,
            ));
        }
        Ok(Self::StationAccessPoint { plan, request })
    }

    /// Join a checked standalone-monitor topology to its runtime policy.
    pub fn standalone_monitor(
        plan: WifiPlan,
        request: MonitorRequest,
    ) -> Result<Self, WifiServiceRequestFailure<MonitorRequest>> {
        let Some(checked) = plan.standalone_monitor() else {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::NotStandaloneMonitorTopology,
            ));
        };
        if checked.monitor() != request.monitor_config() {
            return Err(WifiServiceRequestFailure::new(
                request,
                WifiServiceRequestError::MonitorPolicyMismatch,
            ));
        }
        Ok(Self::StandaloneMonitor { plan, request })
    }

    pub const fn plan(&self) -> WifiPlan {
        match self {
            Self::StandaloneScan { plan, .. }
            | Self::Station { plan, .. }
            | Self::AccessPoint { plan, .. }
            | Self::StationAccessPoint { plan, .. }
            | Self::StandaloneMonitor { plan, .. } => *plan,
        }
    }

    pub const fn scan_request(&self) -> Option<&WifiScanRequest> {
        match self {
            Self::StandaloneScan { request, .. } => Some(request),
            Self::Station { .. }
            | Self::AccessPoint { .. }
            | Self::StationAccessPoint { .. }
            | Self::StandaloneMonitor { .. } => None,
        }
    }

    pub const fn station_request(&self) -> Option<&StationRequest> {
        match self {
            Self::StandaloneScan { .. } => None,
            Self::Station { request, .. } => Some(request),
            Self::StationAccessPoint { request, .. } => Some(request.station()),
            Self::AccessPoint { .. } | Self::StandaloneMonitor { .. } => None,
        }
    }

    pub const fn access_point_request(&self) -> Option<&AccessPointRequest> {
        match self {
            Self::AccessPoint { request, .. } => Some(request),
            Self::StationAccessPoint { request, .. } => Some(request.access_point()),
            Self::StandaloneScan { .. } | Self::Station { .. } | Self::StandaloneMonitor { .. } => {
                None
            }
        }
    }

    pub const fn monitor_request(&self) -> Option<&MonitorRequest> {
        match self {
            Self::StandaloneScan { .. }
            | Self::Station { .. }
            | Self::AccessPoint { .. }
            | Self::StationAccessPoint { .. } => None,
            Self::StandaloneMonitor { request, .. } => Some(request),
        }
    }

    pub const fn station_access_point_request(&self) -> Option<&StationAccessPointRequest> {
        match self {
            Self::StationAccessPoint { request, .. } => Some(request),
            Self::StandaloneScan { .. }
            | Self::Station { .. }
            | Self::AccessPoint { .. }
            | Self::StandaloneMonitor { .. } => None,
        }
    }
}

impl fmt::Debug for WifiServiceRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StandaloneScan { plan, request } => formatter
                .debug_struct("StandaloneScan")
                .field("plan", plan)
                .field("request", request)
                .finish(),
            Self::Station { plan, request } => formatter
                .debug_struct("Station")
                .field("plan", plan)
                .field("request", request)
                .finish(),
            Self::AccessPoint { plan, request } => formatter
                .debug_struct("AccessPoint")
                .field("plan", plan)
                .field("request", request)
                .finish(),
            Self::StationAccessPoint { plan, request } => formatter
                .debug_struct("StationAccessPoint")
                .field("plan", plan)
                .field("request", request)
                .finish(),
            Self::StandaloneMonitor { plan, request } => formatter
                .debug_struct("StandaloneMonitor")
                .field("plan", plan)
                .field("request", request)
                .finish(),
        }
    }
}

/// Runtime request does not match its independently checked owner topology.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiServiceRequestError {
    MissingScanTopology,
    MissingStationTopology,
    MissingAccessPointTopology,
    UnexpectedTopologyRole,
    NotStandaloneMonitorTopology,
    MonitorPolicyMismatch,
    /// The station associated on a different physical channel than the
    /// requested same-channel SoftAP. Production does not time-slice the two
    /// roles or silently retune an associated station.
    StationAccessPointChannelMismatch,
}

impl fmt::Display for WifiServiceRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingScanTopology => {
                "standalone scan requires a checked station-capable topology"
            }
            Self::MissingStationTopology => {
                "station service request requires a checked station topology"
            }
            Self::MissingAccessPointTopology => {
                "access-point service request requires a checked access-point topology"
            }
            Self::UnexpectedTopologyRole => {
                "station-only service request cannot contain another active Wi-Fi role"
            }
            Self::NotStandaloneMonitorTopology => {
                "standalone monitor request requires an exclusive checked monitor topology"
            }
            Self::MonitorPolicyMismatch => {
                "monitor runtime policy differs from the checked monitor topology"
            }
            Self::StationAccessPointChannelMismatch => {
                "station association channel differs from the requested access-point channel"
            }
        })
    }
}

impl core::error::Error for WifiServiceRequestError {}

/// Request/topology mismatch retaining the exact role request.
#[derive(Debug)]
pub struct WifiServiceRequestFailure<R> {
    pub request: R,
    pub error: WifiServiceRequestError,
}

impl<R> WifiServiceRequestFailure<R> {
    const fn new(request: R, error: WifiServiceRequestError) -> Self {
        Self { request, error }
    }
}

/// Wi-Fi roles for which a supervisor composition owns complete static
/// resources.
///
/// This is not the currently active topology. In particular, provisioning
/// both station and standalone monitor allows sequential STA -> stopped ->
/// monitor transitions even on hardware which cannot run both concurrently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiSupervisorConfiguration {
    capabilities: MacServiceCapabilities,
    station: Option<WifiStationConfig>,
    access_point: Option<WifiAccessPointConfig>,
    standalone_scan: bool,
    standalone_monitor: bool,
}

impl WifiSupervisorConfiguration {
    pub const fn new(capabilities: MacServiceCapabilities) -> Self {
        Self {
            capabilities,
            station: None,
            access_point: None,
            standalone_scan: false,
            standalone_monitor: false,
        }
    }

    pub const fn with_station(mut self, station: WifiStationConfig) -> Self {
        self.station = Some(station);
        self
    }

    pub const fn with_access_point(mut self, access_point: WifiAccessPointConfig) -> Self {
        self.access_point = Some(access_point);
        self
    }

    pub const fn with_standalone_scan(mut self) -> Self {
        self.standalone_scan = true;
        self
    }

    pub fn plan_scan(
        self,
        request: WifiScanRequest,
    ) -> Result<WifiServiceRequest, WifiServicePlanningFailure<WifiScanRequest>> {
        if !self.standalone_scan {
            return Err(WifiServicePlanningFailure {
                request,
                error: WifiServicePlanningError::ScanNotProvisioned,
            });
        }
        let Some(station) = self.station else {
            return Err(WifiServicePlanningFailure {
                request,
                error: WifiServicePlanningError::StationNotProvisioned,
            });
        };
        let plan = match WifiConfig::station(station).validate(self.capabilities) {
            Ok(plan) => plan,
            Err(error) => {
                return Err(WifiServicePlanningFailure {
                    request,
                    error: WifiServicePlanningError::Topology(error),
                });
            }
        };
        WifiServiceRequest::standalone_scan(plan, request).map_err(|failure| {
            WifiServicePlanningFailure {
                request: failure.request,
                error: WifiServicePlanningError::Request(failure.error),
            }
        })
    }

    pub const fn with_standalone_monitor(mut self) -> Self {
        self.standalone_monitor = true;
        self
    }

    #[allow(
        clippy::result_large_err,
        reason = "a rejected no-alloc station request must return its exact credential owner"
    )]
    pub fn plan_station(
        self,
        request: StationRequest,
    ) -> Result<WifiServiceRequest, WifiServicePlanningFailure<StationRequest>> {
        let Some(station) = self.station else {
            return Err(WifiServicePlanningFailure {
                request,
                error: WifiServicePlanningError::StationNotProvisioned,
            });
        };
        let plan = match WifiConfig::station(station).validate(self.capabilities) {
            Ok(plan) => plan,
            Err(error) => {
                return Err(WifiServicePlanningFailure {
                    request,
                    error: WifiServicePlanningError::Topology(error),
                });
            }
        };
        WifiServiceRequest::station(plan, request).map_err(|failure| WifiServicePlanningFailure {
            request: failure.request,
            error: WifiServicePlanningError::Request(failure.error),
        })
    }

    pub fn plan_access_point(
        self,
        request: AccessPointRequest,
    ) -> Result<WifiServiceRequest, WifiServicePlanningFailure<AccessPointRequest>> {
        let Some(access_point) = self.access_point else {
            return Err(WifiServicePlanningFailure {
                request,
                error: WifiServicePlanningError::AccessPointNotProvisioned,
            });
        };
        let plan = match WifiConfig::access_point(access_point).validate(self.capabilities) {
            Ok(plan) => plan,
            Err(error) => {
                return Err(WifiServicePlanningFailure {
                    request,
                    error: WifiServicePlanningError::Topology(error),
                });
            }
        };
        WifiServiceRequest::access_point(plan, request).map_err(|failure| {
            WifiServicePlanningFailure {
                request: failure.request,
                error: WifiServicePlanningError::Request(failure.error),
            }
        })
    }

    #[allow(
        clippy::result_large_err,
        reason = "planning is no-alloc and must return the untouched combined credential owner"
    )]
    pub fn plan_station_access_point(
        self,
        request: StationAccessPointRequest,
    ) -> Result<WifiServiceRequest, WifiServicePlanningFailure<StationAccessPointRequest>> {
        let Some(station) = self.station else {
            return Err(WifiServicePlanningFailure {
                request,
                error: WifiServicePlanningError::StationNotProvisioned,
            });
        };
        let Some(access_point) = self.access_point else {
            return Err(WifiServicePlanningFailure {
                request,
                error: WifiServicePlanningError::AccessPointNotProvisioned,
            });
        };
        let plan = match WifiConfig::station_access_point(station, access_point)
            .validate(self.capabilities)
        {
            Ok(plan) => plan,
            Err(error) => {
                return Err(WifiServicePlanningFailure {
                    request,
                    error: WifiServicePlanningError::Topology(error),
                });
            }
        };
        WifiServiceRequest::station_access_point(plan, request).map_err(|failure| {
            WifiServicePlanningFailure {
                request: failure.request,
                error: WifiServicePlanningError::Request(failure.error),
            }
        })
    }

    pub fn plan_monitor(
        self,
        request: MonitorRequest,
    ) -> Result<WifiServiceRequest, WifiServicePlanningFailure<MonitorRequest>> {
        if !self.standalone_monitor {
            return Err(WifiServicePlanningFailure {
                request,
                error: WifiServicePlanningError::MonitorNotProvisioned,
            });
        }
        let plan = match WifiConfig::monitor(request.monitor_config()).validate(self.capabilities) {
            Ok(plan) => plan,
            Err(error) => {
                return Err(WifiServicePlanningFailure {
                    request,
                    error: WifiServicePlanningError::Topology(error),
                });
            }
        };
        WifiServiceRequest::standalone_monitor(plan, request).map_err(|failure| {
            WifiServicePlanningFailure {
                request: failure.request,
                error: WifiServicePlanningError::Request(failure.error),
            }
        })
    }
}

/// Why a provisioned supervisor cannot construct a checked service request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiServicePlanningError {
    ScanNotProvisioned,
    StationNotProvisioned,
    AccessPointNotProvisioned,
    MonitorNotProvisioned,
    Topology(WifiConfigError),
    Request(WifiServiceRequestError),
}

/// Planning failure retaining the untouched application request.
#[derive(Debug)]
pub struct WifiServicePlanningFailure<R> {
    pub request: R,
    pub error: WifiServicePlanningError,
}

#[cfg(test)]
mod tests;
