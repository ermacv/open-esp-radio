//! Finite same-channel STA plus SoftAP DATAPATH ownership policy.
//!
//! Register qualification alone cannot prove that the two roles preserve one
//! another while sharing the physical MAC. This state machine is the small,
//! allocation-free policy boundary between independently reviewed register
//! leaves and the role-neutral runtime orchestrator. It deliberately performs
//! no MMIO and does not claim scheduler or DMA ownership.

use oer_esp32s31_ieee80211_mac::sta_ap_registers::{
    StaApRegisterHardware, configure_sta_ap_receive_registers,
    disable_access_point_receive_registers, disable_station_receive_registers,
};
use oer_ieee80211_mac::channel::WifiChannel;

/// Addresses required only when the second role joins the shared MAC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaApReceiveIdentities {
    pub station_address: [u8; 6],
    pub station_bssid: [u8; 6],
    pub access_point_address: [u8; 6],
}

/// Exact register consequence of one already-validated lifecycle edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApRegisterAction {
    None,
    ConfigureBoth(StaApReceiveIdentities),
    DisableStationPreserveAccessPoint,
    DisableAccessPointPreserveStation,
}

pub const fn sta_ap_register_action(
    transition: StaApTransition,
    identities: StaApReceiveIdentities,
) -> StaApRegisterAction {
    match transition {
        StaApTransition::StartStationPreserveAccessPoint
        | StaApTransition::StartAccessPointPreserveStation => {
            StaApRegisterAction::ConfigureBoth(identities)
        }
        StaApTransition::StopStationPreserveAccessPoint => {
            StaApRegisterAction::DisableStationPreserveAccessPoint
        }
        StaApTransition::StopAccessPointPreserveStation => {
            StaApRegisterAction::DisableAccessPointPreserveStation
        }
        StaApTransition::StartStationCold
        | StaApTransition::StartAccessPointCold
        | StaApTransition::StopStationLastRole
        | StaApTransition::StopAccessPointLastRole => StaApRegisterAction::None,
    }
}

/// Apply the finite register half of a same-channel lifecycle transition.
///
/// Cold single-role entry/exit remains owned by its existing transaction.
/// This function handles only edges that must preserve the other live role.
pub fn apply_sta_ap_register_action<H: StaApRegisterHardware>(
    hardware: &mut H,
    action: StaApRegisterAction,
) {
    match action {
        StaApRegisterAction::None => {}
        StaApRegisterAction::ConfigureBoth(identities) => configure_sta_ap_receive_registers(
            hardware,
            identities.station_address,
            identities.station_bssid,
            identities.access_point_address,
        ),
        StaApRegisterAction::DisableStationPreserveAccessPoint => {
            disable_station_receive_registers(hardware);
        }
        StaApRegisterAction::DisableAccessPointPreserveStation => {
            disable_access_point_receive_registers(hardware);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApRole {
    Station,
    AccessPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApLifecycleState {
    Idle,
    Station { channel: WifiChannel },
    AccessPoint { channel: WifiChannel },
    Concurrent { channel: WifiChannel },
}

impl StaApLifecycleState {
    pub const fn channel(self) -> Option<WifiChannel> {
        match self {
            Self::Idle => None,
            Self::Station { channel }
            | Self::AccessPoint { channel }
            | Self::Concurrent { channel } => Some(channel),
        }
    }

    pub const fn station_active(self) -> bool {
        matches!(self, Self::Station { .. } | Self::Concurrent { .. })
    }

    pub const fn access_point_active(self) -> bool {
        matches!(self, Self::AccessPoint { .. } | Self::Concurrent { .. })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApTransition {
    StartStationCold,
    StartStationPreserveAccessPoint,
    StartAccessPointCold,
    StartAccessPointPreserveStation,
    StopStationLastRole,
    StopStationPreserveAccessPoint,
    StopAccessPointLastRole,
    StopAccessPointPreserveStation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApLifecycleError {
    RoleAlreadyActive(StaApRole),
    RoleInactive(StaApRole),
    ChannelConflict {
        active: WifiChannel,
        requested: WifiChannel,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaApLifecycle {
    state: StaApLifecycleState,
}

impl StaApLifecycle {
    pub const fn new() -> Self {
        Self {
            state: StaApLifecycleState::Idle,
        }
    }

    pub const fn state(&self) -> StaApLifecycleState {
        self.state
    }

    pub fn start_station(
        &mut self,
        channel: WifiChannel,
    ) -> Result<StaApTransition, StaApLifecycleError> {
        let (state, transition) = match self.state {
            StaApLifecycleState::Idle => (
                StaApLifecycleState::Station { channel },
                StaApTransition::StartStationCold,
            ),
            StaApLifecycleState::AccessPoint { channel: active } if active == channel => (
                StaApLifecycleState::Concurrent { channel },
                StaApTransition::StartStationPreserveAccessPoint,
            ),
            StaApLifecycleState::AccessPoint { channel: active } => {
                return Err(StaApLifecycleError::ChannelConflict {
                    active,
                    requested: channel,
                });
            }
            StaApLifecycleState::Station { .. } | StaApLifecycleState::Concurrent { .. } => {
                return Err(StaApLifecycleError::RoleAlreadyActive(StaApRole::Station));
            }
        };
        self.state = state;
        Ok(transition)
    }

    pub fn start_access_point(
        &mut self,
        channel: WifiChannel,
    ) -> Result<StaApTransition, StaApLifecycleError> {
        let (state, transition) = match self.state {
            StaApLifecycleState::Idle => (
                StaApLifecycleState::AccessPoint { channel },
                StaApTransition::StartAccessPointCold,
            ),
            StaApLifecycleState::Station { channel: active } if active == channel => (
                StaApLifecycleState::Concurrent { channel },
                StaApTransition::StartAccessPointPreserveStation,
            ),
            StaApLifecycleState::Station { channel: active } => {
                return Err(StaApLifecycleError::ChannelConflict {
                    active,
                    requested: channel,
                });
            }
            StaApLifecycleState::AccessPoint { .. } | StaApLifecycleState::Concurrent { .. } => {
                return Err(StaApLifecycleError::RoleAlreadyActive(
                    StaApRole::AccessPoint,
                ));
            }
        };
        self.state = state;
        Ok(transition)
    }

    pub fn stop_station(&mut self) -> Result<StaApTransition, StaApLifecycleError> {
        let (state, transition) = match self.state {
            StaApLifecycleState::Station { .. } => (
                StaApLifecycleState::Idle,
                StaApTransition::StopStationLastRole,
            ),
            StaApLifecycleState::Concurrent { channel } => (
                StaApLifecycleState::AccessPoint { channel },
                StaApTransition::StopStationPreserveAccessPoint,
            ),
            StaApLifecycleState::Idle | StaApLifecycleState::AccessPoint { .. } => {
                return Err(StaApLifecycleError::RoleInactive(StaApRole::Station));
            }
        };
        self.state = state;
        Ok(transition)
    }

    pub fn stop_access_point(&mut self) -> Result<StaApTransition, StaApLifecycleError> {
        let (state, transition) = match self.state {
            StaApLifecycleState::AccessPoint { .. } => (
                StaApLifecycleState::Idle,
                StaApTransition::StopAccessPointLastRole,
            ),
            StaApLifecycleState::Concurrent { channel } => (
                StaApLifecycleState::Station { channel },
                StaApTransition::StopAccessPointPreserveStation,
            ),
            StaApLifecycleState::Idle | StaApLifecycleState::Station { .. } => {
                return Err(StaApLifecycleError::RoleInactive(StaApRole::AccessPoint));
            }
        };
        self.state = state;
        Ok(transition)
    }
}

impl Default for StaApLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
