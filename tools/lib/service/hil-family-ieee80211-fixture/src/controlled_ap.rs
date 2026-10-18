//! Lifetime-safe access to the repository-controlled HIL access point.

use crate::Result;
use oer_hil_lab::config::{StationConfig, StationFixtureConfig};
use oer_hil_scenario_catalog::link::{
    AccessPointBeacon, AccessPointSecurity, ManagementFrameProtection, PhyExpectation,
};

/// Restores the selected AP frontier on every normal or error return.
pub enum ControlledAp {
    Local(super::local::ap::AccessPoint),
    OpenWrt(Box<super::openwrt::ap::AccessPoint>),
    External,
}

impl ControlledAp {
    pub fn start(
        station: &StationConfig,
        fixture: &StationFixtureConfig,
        phy: PhyExpectation,
        management_frame_protection: ManagementFrameProtection,
        access_point_security: AccessPointSecurity,
        beacon: Option<AccessPointBeacon>,
    ) -> Result<Self> {
        if beacon.is_some() && !matches!(fixture, StationFixtureConfig::OpenWrt(_)) {
            return Err(crate::Error::new(
                "a scenario beacon schedule requires the OpenWrt station fixture",
            )
            .into());
        }
        if (management_frame_protection.negotiated() || access_point_security.offers_sae())
            && !matches!(fixture, StationFixtureConfig::OpenWrt(_))
        {
            return Err(crate::Error::new(
                "management frame protection and WPA3 require the OpenWrt station fixture",
            )
            .into());
        }
        match fixture {
            StationFixtureConfig::LocalLinux(config) => Ok(Self::Local(
                super::local::ap::AccessPoint::start(config, station, phy)
                    .map_err(super::Error::context)?,
            )),
            StationFixtureConfig::OpenWrt(openwrt) => Ok(Self::OpenWrt(Box::new(
                super::openwrt::ap::AccessPoint::start(
                    openwrt,
                    station,
                    phy,
                    management_frame_protection,
                    access_point_security,
                    beacon,
                )
                .map_err(super::Error::context)?,
            ))),
            StationFixtureConfig::External(_) => {
                require_station_credentials(station)?;
                Ok(Self::External)
            }
        }
    }

    pub fn stop(&mut self) -> Result<()> {
        match self {
            Self::Local(ap) => ap.stop(),
            Self::OpenWrt(ap) => ap.stop(),
            Self::External => Err(crate::Error::new(
                "an external station fixture cannot be stopped by HIL",
            )
            .into()),
        }
    }

    pub fn restart(&mut self) -> Result<()> {
        match self {
            Self::Local(ap) => ap.restart(),
            Self::OpenWrt(ap) => ap.restart(),
            Self::External => Err(crate::Error::new(
                "an external station fixture cannot be restarted by HIL",
            )
            .into()),
        }
    }
}

pub fn require_station_credentials(station: &StationConfig) -> Result<()> {
    let _credentials = station.credentials();
    Ok(())
}
