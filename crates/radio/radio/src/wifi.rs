//! Wi-Fi configuration, service requests and affine role lifecycle.
//!
//! These contracts are independent of the executor used to drive the radio.

mod config;
pub mod requests;
pub mod supervisor;

pub use config::{
    WifiAccessPointConfig, WifiConfig, WifiConfigError, WifiMacAddress, WifiMacAddressError,
    WifiMonitorConfig, WifiPlan, WifiStandaloneEspNowPlan, WifiStandaloneMonitorPlan,
    WifiStationConfig,
};
pub use oer_ieee80211_rsn::{Pmk, sae::SaePassword};
pub use oer_ieee80211_softmac::{
    MONITOR_CHANNEL_SEQUENCE_CAPACITY, MacRxEvidence, MonitorChannelPolicy, MonitorChannelSequence,
    MonitorChannelSequenceError, MonitorDropReason, MonitorFilter, MonitorFrame, MonitorFrameType,
    MonitorFrameTypeMask, MonitorPublishOutcome, MonitorSink,
};
pub use oer_ieee80211_sta::station::{
    StaFaultCause, StaFaultPhase, StaFaultReason, StaLifecycleStage, StaReconnectPolicy,
};
pub use requests::{
    AccessPointBeaconInterval, AccessPointBeaconIntervalError, AccessPointClientLimit,
    AccessPointClientLimitError, AccessPointDtimPeriod, AccessPointDtimPeriodError,
    AccessPointInactiveTimeout, AccessPointInactiveTimeoutError, AccessPointRequest,
    AccessPointRequestError, AccessPointSecurity, MonitorCapturePolicy, MonitorRequest,
    StandaloneEspNowPeerError, StandaloneEspNowRequest, StationAccessPointRequest,
    StationCredentialError, StationDiscovery, StationListenInterval, StationPowerMode,
    StationRequest, StationScanChannelIter, StationScanChannelOrderIter, StationScanChannels,
    StationScanChannelsError, StationScanPolicy, StationSecurity, WifiScanRequest,
    WifiServicePlanningError, WifiServicePlanningFailure, WifiServiceRequest,
    WifiServiceRequestError, WifiServiceRequestFailure, WifiSupervisorConfiguration,
};
pub use supervisor::{
    RadioController, RadioSubsystemGeneration, WIFI_SCAN_RESULT_CAPACITY, WifiAccessPoint,
    WifiIdle, WifiMonitor, WifiRadioRestartReport, WifiRadioRestartRf, WifiRoleStartFailure,
    WifiRoleStopFailure, WifiScanCompleted, WifiScanFailure, WifiScanOperationFailure,
    WifiScanReport, WifiScanResult, WifiStartFailure, WifiStartReport, WifiStartResult,
    WifiStation, WifiStationAccessPoint, WifiStopReport, WifiSupervisorPort,
};
pub use {
    oer_ieee80211_mac::channel::WifiChannel, oer_ieee80211_mac::channel::WifiChannelError,
    oer_ieee80211_mac::channel::WifiChannelWidth, oer_ieee80211_mac::security::LinkProtection,
    oer_ieee80211_mac::ssid::WifiSsid, oer_ieee80211_mac::ssid::WifiSsidError,
    oer_ieee80211_mac::station::association::Preference,
};

/// Synthetic portable service profiles for this crate's and adapters' tests.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support;
