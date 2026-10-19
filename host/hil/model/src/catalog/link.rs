//! Wi-Fi link vocabulary shared by the laboratory configuration and the
//! scenarios that select a fixture link.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PhyExpectation {
    /// A non-HT BSS: the access point advertises no HT or HE elements.
    Legacy,
    He20,
    Ht20,
    Ht40,
}

impl PhyExpectation {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::He20 => "he20",
            Self::Ht20 => "ht20",
            Self::Ht40 => "ht40",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HtGuardIntervalExpectation {
    #[default]
    Any,
    Long,
    Short,
}

impl HtGuardIntervalExpectation {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Long => "long",
            Self::Short => "short",
        }
    }
}

/// Management frame protection the station fixture's access point offers.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManagementFrameProtection {
    #[default]
    Disabled,
    /// The access point is capable of protection (MFPC).
    Optional,
    /// The access point requires protection (MFPC and MFPR).
    Required,
}

impl ManagementFrameProtection {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Optional => "optional",
            Self::Required => "required",
        }
    }

    pub const fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
    }

    /// Whether a station capable of protection negotiates it with this
    /// access point.
    pub const fn negotiated(self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

/// The personal security the station fixture's access point offers.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccessPointSecurity {
    #[default]
    Wpa2Personal,
    /// SAE only, with management frame protection required.
    Wpa3Personal,
    /// PSK and SAE side by side, with management frame protection capable.
    Wpa3Transition,
}

impl AccessPointSecurity {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Wpa2Personal => "wpa2-personal",
            Self::Wpa3Personal => "wpa3-personal",
            Self::Wpa3Transition => "wpa3-transition",
        }
    }

    pub const fn is_wpa2_personal(&self) -> bool {
        matches!(self, Self::Wpa2Personal)
    }

    /// Whether a WPA2-Personal station, which upgrades to SAE, authenticates
    /// with SAE.
    pub const fn offers_sae(self) -> bool {
        !self.is_wpa2_personal()
    }
}

/// The beacon schedule a scenario sets on the station fixture's access
/// point: its beacon interval in time units and its DTIM period in beacons.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessPointBeacon {
    pub interval_tu: u16,
    pub dtim_period: u8,
}

impl AccessPointBeacon {
    /// hostapd's accepted ranges: a beacon interval of 15..=65535 TU and a
    /// DTIM period of 1..=255 beacons.
    pub fn validate(self) -> Result<(), String> {
        if self.interval_tu < 15 {
            return Err(format!(
                "access_point_beacon.interval_tu {} is below hostapd's 15 TU",
                self.interval_tu
            ));
        }
        if self.dtim_period == 0 {
            return Err("access_point_beacon.dtim_period must be at least 1".into());
        }
        Ok(())
    }
}

/// How one scenario uses the shared Wi-Fi laboratory.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WifiLabUse {
    /// The link the station fixture must provide, when the scenario names one.
    pub link: Option<PhyExpectation>,
    /// Management frame protection of the station fixture's access point.
    pub management_frame_protection: ManagementFrameProtection,
    /// Personal security of the station fixture's access point.
    pub access_point_security: AccessPointSecurity,
    /// The target runs the access point; the fixture follows its channel.
    pub access_point: bool,
    /// The beacon schedule the station fixture's access point must run,
    /// restored after the scenario.
    pub access_point_beacon: Option<AccessPointBeacon>,
}
