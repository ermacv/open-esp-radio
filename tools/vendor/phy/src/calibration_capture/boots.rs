//! The typed captures of one cross-check: each vendor and production boot
//! as the workload records it in the run bundle, and the lifecycle point at
//! which both sides report their state.
use crate::calibration_capture::Result;
use crate::calibration_capture::space::Space;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Lifecycle point at which both sides report their state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Lifecycle {
    /// After the cold calibration and the Wi-Fi bring-up.
    #[default]
    Cold,
    /// After one further Wi-Fi radio restart: RF closed and woken.
    Restart,
    /// After the IEEE 802.15.4 radio is configured and receives, before
    /// any transmission.
    Ieee802154,
    /// As `I`, after one IEEE 802.15.4 disable and enable (the
    /// vendor driver's OFF and ON, production's session stop and start).
    Ieee802154Restart,
}

impl Lifecycle {
    /// Whether the IEEE 802.15.4 radio, rather than Wi-Fi, holds the PHY.
    pub fn ieee802154(self) -> bool {
        matches!(self, Self::Ieee802154 | Self::Ieee802154Restart)
    }

    /// The vendor firmware project of the chip's vendor calibration
    /// projects (its profile's `[hil] vendor-calibration`) the point runs.
    pub fn vendor_project(self) -> &'static str {
        if self.ieee802154() {
            "ieee802154-reference"
        } else {
            "calibration"
        }
    }
}

/// Register values one boot read, by address, and the registers whose read
/// reset or hung the chip.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Readings {
    pub values: BTreeMap<u32, u32>,
    pub unreadable: BTreeSet<u32>,
}

impl Readings {
    /// The complete replies and unreadable records of `console` in `space`,
    /// in the vendor calibration firmware's line format.
    pub fn parse(space: Space, console: &str) -> Result<Self> {
        Ok(Self {
            values: space.replies(console)?,
            unreadable: crate::calibration_capture::vendor::unreadable(console)?,
        })
    }
}

/// One vendor boot.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct VendorBoot {
    /// The vendor calibration object (the chip comparison's vendor object)
    /// the boot reported; the IEEE 802.15.4 reference firmware reports none.
    pub calibration: Option<Vec<u8>>,
    /// The radio-PHY register image.
    pub registers: Readings,
    /// The analog image.
    pub analog: Readings,
    /// Further device windows the reference firmware read, when asked: a
    /// vendor-side investigation, never compared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<Readings>,
    /// The analog image and windows after one transmitted frame, when asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transmitted: Option<Transmitted>,
}

/// The vendor reads after one transmitted frame.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Transmitted {
    pub analog: Readings,
    pub windows: Option<Readings>,
}

/// One production boot.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProductionBoot {
    /// The relation's output bytes projected from the retained calibration
    /// the boot published;
    /// none at an IEEE 802.15.4 point, whose session publishes no startup artifact.
    pub calibration: Option<Vec<u8>>,
    pub registers: Readings,
    pub analog: Readings,
}

/// What both sides ran.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Images {
    pub started_unix_seconds: u64,
    pub vendor_project: String,
    pub vendor_application_sha256: String,
    pub vendor_idf_revision: String,
    /// The production image class.
    pub production_image: String,
    pub production_application_sha256: String,
}
