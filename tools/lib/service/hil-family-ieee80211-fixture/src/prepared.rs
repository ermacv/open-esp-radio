//! Scenario-level controlled AP fixture for station-network workloads.
//!
//! Callers run the scenario's fixture preflight before [`Prepared::start`].

use super::controlled_ap::ControlledAp;
use crate::Result;
use oer_hil_lab::config::{LabConfig, StationFixtureConfig};
use oer_hil_run_bundle_format::run::fixtures::{self, Applied, BeaconProtection, Protection};
use oer_hil_scenario::Plan;
use std::{
    cell::{RefCell, RefMut},
    path::Path,
};

pub struct Prepared {
    /// Declared first so it drops before the AP: the fixture BSS outlives
    /// its peer.
    _peer: Option<ProtectionPeer>,
    ap: Option<RefCell<ControlledAp>>,
}

/// A laboratory peer that obliges the station fixture AP to protect.
/// Owners restore the laptop radio when dropped.
enum ProtectionPeer {
    NonHtMember {
        _client: super::local::client::ControlledClient,
    },
    LegacyBss {
        _bss: super::local::legacy_bss::LegacyBss,
    },
}

/// How long one beacon window lasts, and how many windows the AP may take
/// to reflect a new peer.
const BEACON_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);
const BEACON_WINDOWS: u32 = 5;

impl Prepared {
    pub fn start(lab: &LabConfig, plan: &Plan, output: &Path) -> Result<Self> {
        let required = plan.requirements;
        let ap = if required.station_network {
            let phy = lab.fixture_phy(plan.wifi);
            let mut ap = ControlledAp::start(
                &lab.station,
                &lab.station_fixture,
                phy,
                plan.wifi.management_frame_protection,
                plan.wifi.access_point_security,
                plan.wifi.access_point_beacon,
            )
            .map_err(super::Error::context)?;
            // A fresh selected-radio epoch is part of the multi-client fixture.
            // It never resets the router's other radios or the wired uplink.
            if required.openwrt_client {
                ap.stop()?;
                ap.restart()?;
            }
            let applied = match &ap {
                ControlledAp::OpenWrt(owner) => Some(Applied::OpenWrt(owner.report()?)),
                ControlledAp::Local(owner) => Some(Applied::Local(owner.report())),
                ControlledAp::External => None,
            };
            if let Some(applied) = applied {
                fixtures::write(output, &applied)?;
            }
            Some(RefCell::new(ap))
        } else {
            None
        };
        let peer = protection_peer(lab, plan, output)?;
        Ok(Self { _peer: peer, ap })
    }

    pub fn ap(&self) -> Result<RefMut<'_, ControlledAp>> {
        self.ap
            .as_ref()
            .ok_or_else(|| "scenario did not prepare an AP fixture".into())
            .map(RefCell::borrow_mut)
    }
}

fn protection_peer(lab: &LabConfig, plan: &Plan, output: &Path) -> Result<Option<ProtectionPeer>> {
    let required = plan.requirements;
    if !required.non_ht_member && !required.legacy_bss {
        return Ok(None);
    }
    let StationFixtureConfig::OpenWrt(ap) = &lab.station_fixture else {
        return Err(super::Error::new(
            "induced BSS protection requires the OpenWrt station fixture and a free laptop radio",
        )
        .into());
    };
    let record = output;
    let output = output.join("protection-peer");
    std::fs::create_dir_all(&output)?;
    let peer = if required.non_ht_member {
        ProtectionPeer::NonHtMember {
            _client: super::local::client::ControlledClient::connect(
                &super::local::client::ClientNetwork::station_fixture(
                    &lab.station,
                    ap.channel,
                    super::local::client::ClientPhy::NonHt,
                ),
                &output,
            )?,
        }
    } else {
        let config = lab.legacy_bss.as_ref().ok_or_else(|| {
            super::Error::new("scenario requires the [legacy_bss] laboratory capability")
        })?;
        ProtectionPeer::LegacyBss {
            _bss: super::local::legacy_bss::LegacyBss::start(config, ap.channel, &output)?,
        }
    };
    let bssid = super::openwrt::air_monitor::fixture_bssid(ap)?;
    let mut observed = Vec::new();
    for window in 0..BEACON_WINDOWS {
        let beacons = super::openwrt::air_monitor::observe_beacons(
            lab,
            bssid,
            BEACON_WINDOW,
            &output.join(format!("beacons-{window}")),
        )?;
        let protection = from_beacons(&beacons);
        observed.push(protection);
        if satisfies(protection, required) {
            fixtures::write(
                record,
                &Protection {
                    schema: 1,
                    bssid: bssid.to_string(),
                    established: None,
                    non_ht_member: required.non_ht_member,
                    legacy_bss: required.legacy_bss,
                    windows: observed,
                },
            )?;
            return Ok(Some(peer));
        }
    }
    fixtures::write(
        record,
        &Protection {
            schema: 1,
            bssid: bssid.to_string(),
            established: Some(false),
            non_ht_member: required.non_ht_member,
            legacy_bss: required.legacy_bss,
            windows: observed.clone(),
        },
    )?;
    Err(super::Error::new(format!(
        "the station fixture AP did not advertise the induced protection within {} s: {observed:?}",
        BEACON_WINDOW.as_secs() * u64::from(BEACON_WINDOWS)
    ))
    .into())
}

/// The protection fields of every beacon in one observation window.
fn from_beacons(beacons: &[crate::air::AirFrame]) -> BeaconProtection {
    let ht_protection = beacons
        .first()
        .and_then(|beacon| beacon.ht_protection)
        .filter(|mode| {
            beacons
                .iter()
                .all(|beacon| beacon.ht_protection == Some(*mode))
        });
    BeaconProtection {
        beacons: beacons.len(),
        erp_use_protection: !beacons.is_empty()
            && beacons
                .iter()
                .all(|beacon| beacon.erp_information.is_some_and(|erp| erp & 0x02 != 0)),
        ht_protection,
    }
}

/// A non-HT member requires non-HT mixed HT protection; an overlapping
/// legacy BSS requires ERP Use_Protection.
fn satisfies(
    protection: BeaconProtection,
    required: oer_hil_scenario_catalog::requirements::Requirements,
) -> bool {
    protection.beacons != 0
        && (!required.non_ht_member || protection.ht_protection == Some(3))
        && (!required.legacy_bss || protection.erp_use_protection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::air::{FrameKind, test_support::frame};

    fn beacon(erp: u8, ht: u8) -> crate::air::AirFrame {
        let mut beacon = frame(0, FrameKind::BEACON);
        beacon.erp_information = Some(erp);
        beacon.ht_protection = Some(ht);
        beacon
    }

    #[test]
    fn protection_is_established_only_when_every_beacon_carries_it() {
        let non_ht = oer_hil_scenario_catalog::requirements::Requirements {
            non_ht_member: true,
            ..Default::default()
        };
        let legacy = oer_hil_scenario_catalog::requirements::Requirements {
            legacy_bss: true,
            ..Default::default()
        };
        assert!(satisfies(
            from_beacons(&[beacon(0, 3), beacon(0, 3)]),
            non_ht
        ));
        assert!(!satisfies(
            from_beacons(&[beacon(0, 3), beacon(0, 0)]),
            non_ht
        ));
        assert!(!satisfies(from_beacons(&[]), non_ht));
        assert!(satisfies(from_beacons(&[beacon(3, 1)]), legacy));
        assert!(!satisfies(
            from_beacons(&[beacon(2, 1), beacon(0, 1)]),
            legacy
        ));
        assert!(!satisfies(from_beacons(&[beacon(0, 3)]), legacy));
    }
}
