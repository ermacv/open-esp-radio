//! Public application boundary for the ESP32-S31 radio composition.

use crate::{WifiDevice, WifiDevices, monitor::MonitorFrames};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use oer_radio::wifi::{WifiIdle, WifiServicePlanningError};
use oer_radio_supervisor::{WifiStartKind, WifiSupervisorMailbox};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioError {
    Planning(WifiServicePlanningError),
    RoleActive(WifiStartKind),
    HardwareFault,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewError {
    /// The shared PHY could not be prepared for Wi-Fi.
    Phy(oer_esp32s31_radio_runtime::RadioPhyError),
    RadioStart,
    StationRole,
    MacStart,
    StationMemoryInUse,
    RxDmaLayout,
    TxDmaLayout,
    ConnectedResources,
    MonitorResources,
    SupervisorInUse,
}

/// Hardware-free Wi-Fi typestate root for the sole ESP32-S31 radio runner.
pub type WifiControl =
    WifiIdle<WifiSupervisorMailbox<'static, CriticalSectionRawMutex, RadioError>>;

/// Materialized Wi-Fi application resources. The network device and monitor
/// capture stream are independent consumers of the same supervised radio.
pub struct WifiSystem {
    control: WifiControl,
    devices: WifiDevices,
    monitor_frames: MonitorFrames,
    station_status: crate::StationStatus,
    access_point_status: crate::AccessPointStatus,
    #[cfg(feature = "diagnostics")]
    diagnostics: crate::DiagnosticSnapshot,
}

/// Named application capabilities materialized from the Wi-Fi subsystem.
/// PAC, DMA and interrupt state remain exclusively in [`crate::SystemRunner`].
pub struct WifiParts {
    pub control: WifiControl,
    pub station_device: WifiDevice,
    pub access_point_device: WifiDevice,
    pub monitor_frames: MonitorFrames,
    pub station_status: crate::StationStatus,
    pub access_point_status: crate::AccessPointStatus,
    #[cfg(feature = "diagnostics")]
    pub diagnostics: crate::DiagnosticSnapshot,
}

impl WifiSystem {
    pub(super) fn new(
        control: WifiControl,
        devices: WifiDevices,
        monitor_frames: MonitorFrames,
        #[cfg(feature = "diagnostics")] diagnostics: crate::DiagnosticSnapshot,
    ) -> Self {
        Self {
            control,
            devices,
            monitor_frames,
            station_status: crate::StationStatus::new(),
            access_point_status: crate::AccessPointStatus::new(),
            #[cfg(feature = "diagnostics")]
            diagnostics,
        }
    }

    pub fn into_parts(self) -> WifiParts {
        WifiParts {
            control: self.control,
            station_device: self.devices.station,
            access_point_device: self.devices.access_point,
            monitor_frames: self.monitor_frames,
            station_status: self.station_status,
            access_point_status: self.access_point_status,
            #[cfg(feature = "diagnostics")]
            diagnostics: self.diagnostics,
        }
    }
}

/// Wi-Fi started on the shared radio: the application capabilities, the
/// bring-up evidence and the sole owner-holding runner.
pub struct WifiStarted {
    pub wifi: WifiSystem,
    pub initialization: WifiInitialization,
    pub runner: crate::SystemRunner,
}

/// Value-only evidence of Wi-Fi's bring-up on the shared radio, without
/// exposing PHY, register or calibration owners.
pub struct WifiInitialization {
    /// How the shared PHY was prepared for Wi-Fi. Only the first client to
    /// join registers the domain; its registration reports the calibration
    /// path and carries the fresh calibration cache.
    pub phy: oer_esp32s31_radio_runtime::RadioPhyPrepared,
    pub start: oer_esp32s31_ieee80211::mac_start::WifiMacStartReport,
    pub transition: oer_esp32s31_ieee80211::runtime::WifiRuntimeTransitionReport,
}
