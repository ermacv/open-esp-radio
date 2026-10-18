//! The `[wifi]` scenario table: one Wi-Fi workload on a selected firmware
//! image and target data path.
//!
//! Every workload variant carries its own link expectation, observers and
//! acceptance criteria, so a value that has no meaning for a workload cannot
//! be written. Validation covers only the value ranges and the relations
//! that remain between the image, the data path and the workload.

use std::path::Path;

use oer_hil_protocol::wifi::TargetSettings;
use oer_hil_protocol::{
    wifi::WifiApScheduler, wifi::WifiDataPlanePlacement, wifi::WifiRxChecksumPolicy,
    wifi::WifiRxContinuationPolicy, wifi::WifiTxBufferPolicy, wifi::WifiTxUdpChecksumPolicy,
};
use oer_hil_scenario::{AirUse, Plan, ScenarioFamily};
use oer_hil_scenario_catalog::link::{
    AccessPointSecurity, HtGuardIntervalExpectation, ManagementFrameProtection, PhyExpectation,
    WifiLabUse,
};
use oer_hil_scenario_catalog::{bounded, requirements::Requirements};
use oer_hil_schema::image::ImageClass;
use oer_hil_workload::{context::Context, family::Workload, fixture::Fixtures};
use serde::{Deserialize, Serialize};

use crate::Result;
use oer_hil_family_ieee80211_fixture::prepared::Prepared;

pub mod access_point;
mod run;
pub mod station;

pub use access_point::{AccessPoint, AccessPointClients, AccessPointTraffic};
pub use station::{
    AirObservation, InducedProtection, ProtectionPeer, RestartRf, RoleOperation, StationIcmp,
    StationReconnect, StationSleep, StationTcp, StationUdp,
};

// Diagnostic phase totals use u32 cycle accumulators. At 320 MHz, 12 seconds
// leaves margin below the 2^32 wrap point; a 14-second interval cannot.
const CORE0_RX_CYCLE_MAX_DURATION_SECONDS: u16 = 12;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WifiScenario {
    pub image: ImageClass,
    #[serde(default)]
    pub datapath: Datapath,
    pub workload: WifiWorkload,
}

/// Target data-path initialization. Every non-default policy is a
/// diagnostic restricted to the images and workloads that measure it.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Datapath {
    pub placement: WifiDataPlanePlacement,
    pub rx_checksum: WifiRxChecksumPolicy,
    pub tx_udp_checksum: WifiTxUdpChecksumPolicy,
    pub tx_buffer: WifiTxBufferPolicy,
    pub rx_continuation: WifiRxContinuationPolicy,
    pub l1_cache_counters: bool,
}

/// The link a station workload requires, or an AP workload's client link.
///
/// Expectations never change the peer: they only validate target-side
/// observations. Fixture mutations are separate, explicit workload fields.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkExpectation {
    pub phy: PhyExpectation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_mcs: Option<u8>,
    #[serde(default)]
    pub guard_interval: HtGuardIntervalExpectation,
    /// Management frame protection the station fixture offers; the station
    /// must negotiate it whenever it is offered.
    #[serde(
        default,
        skip_serializing_if = "ManagementFrameProtection::is_disabled"
    )]
    pub management_frame_protection: ManagementFrameProtection,
    /// Personal security the station fixture offers; the station must
    /// authenticate with SAE whenever it is offered.
    #[serde(default, skip_serializing_if = "AccessPointSecurity::is_wpa2_personal")]
    pub access_point_security: AccessPointSecurity,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Rx,
    Tx,
    Bidirectional,
}

impl Direction {
    pub const fn receives(self) -> bool {
        matches!(self, Self::Rx | Self::Bidirectional)
    }

    pub const fn transmits(self) -> bool {
        matches!(self, Self::Tx | Self::Bidirectional)
    }
}

/// Offered load in bits per second, as seen from the target: `rx_bps`
/// flows to the target and `tx_bps` from it. The present offers define the
/// traffic direction.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rx_bps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_bps: Option<u64>,
}

impl Offer {
    pub fn direction(self) -> Direction {
        match (self.rx_bps, self.tx_bps) {
            (Some(_), Some(_)) => Direction::Bidirectional,
            (None, Some(_)) => Direction::Tx,
            _ => Direction::Rx,
        }
    }

    fn validate(self) -> Result<()> {
        if self.rx_bps.is_none() && self.tx_bps.is_none() {
            return Err("offer requires rx_bps, tx_bps or both".into());
        }
        for (field, rate) in [("offer.rx_bps", self.rx_bps), ("offer.tx_bps", self.tx_bps)] {
            if let Some(rate) = rate {
                bounded(rate, 1_000, 1_000_000_000, field)?;
            }
        }
        Ok(())
    }

    fn receive_only(self) -> bool {
        self.direction() == Direction::Rx
    }

    fn transmit_only(self) -> bool {
        self.direction() == Direction::Tx
    }
}

/// Floors on the bitrate a transport session achieved.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RateFloors {
    pub minimum_rx_bps: Option<u64>,
    pub minimum_tx_bps: Option<u64>,
    pub minimum_combined_bps: Option<u64>,
}

impl RateFloors {
    /// Each floor needs the offer it measures and cannot exceed it.
    fn validate(self, offer: Offer) -> Result<()> {
        for (field, floor, offered) in [
            ("minimum_rx_bps", self.minimum_rx_bps, offer.rx_bps),
            ("minimum_tx_bps", self.minimum_tx_bps, offer.tx_bps),
        ] {
            if let Some(floor) = floor {
                let offered = offered.ok_or_else(|| format!("{field} requires its offer"))?;
                if floor > offered {
                    return Err(format!("{field} cannot exceed its offer").into());
                }
            }
        }
        if let Some(floor) = self.minimum_combined_bps {
            let (Some(rx), Some(tx)) = (offer.rx_bps, offer.tx_bps) else {
                return Err("minimum_combined_bps requires a bidirectional offer".into());
            };
            if floor > rx.saturating_add(tx) {
                return Err("minimum_combined_bps cannot exceed the RX+TX offer".into());
            }
        }
        Ok(())
    }
}

// Unit variants of an internally tagged enum would ignore unknown keys;
// every variant is a struct so `deny_unknown_fields` stays effective.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum WifiWorkload {
    StationUdp(StationUdp),
    StationTcp(StationTcp),
    StationIcmp(StationIcmp),
    StationSleep(StationSleep),
    StationReconnect(StationReconnect),
    StationApLoss {
        link: LinkExpectation,
        timeout_seconds: u16,
        #[serde(default)]
        require_recovery_echo: bool,
    },
    StationApAbsence {
        link: LinkExpectation,
        timeout_seconds: u16,
        #[serde(default)]
        initially_absent: bool,
    },
    Role {
        link: LinkExpectation,
        timeout_seconds: u16,
        operation: RoleOperation,
    },
    MonitorCapture {
        link: LinkExpectation,
        timeout_seconds: u16,
        duration_seconds: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<u8>,
        snapshot_length: u16,
    },
    AccessPoint(AccessPoint),
    /// One equal-offer UDP session in the selected direction on each endpoint
    /// of a live same-channel STA+AP epoch.
    StationAccessPoint(station::StationAccessPoint),
    /// Controlled loss of the upstream AP tears down the complete same-channel
    /// pair; restoring the fixture permits one explicit fresh paired start.
    StationAccessPointReconnect {
        link: LinkExpectation,
        timeout_seconds: u16,
    },
}

impl WifiWorkload {
    /// The link the station fixture must provide or the AP client uses.
    pub fn link(&self) -> Option<LinkExpectation> {
        match self {
            Self::StationUdp(workload) => Some(workload.link),
            Self::StationTcp(workload) => Some(workload.link),
            Self::StationIcmp(workload) => Some(workload.link),
            Self::StationSleep(workload) => Some(workload.link),
            Self::StationReconnect(workload) => Some(workload.link),
            Self::StationAccessPoint(workload) => Some(workload.link),
            Self::AccessPoint(workload) => workload.link,
            Self::StationApLoss { link, .. }
            | Self::StationApAbsence { link, .. }
            | Self::Role { link, .. }
            | Self::MonitorCapture { link, .. }
            | Self::StationAccessPointReconnect { link, .. } => Some(*link),
        }
    }

    /// Whether the workload measures a receive-only UDP flow.
    fn receive_only_udp(&self) -> bool {
        match self {
            Self::StationUdp(workload) => workload.offer.receive_only(),
            Self::AccessPoint(workload) => workload
                .traffic
                .udp_offer()
                .is_some_and(Offer::receive_only),
            Self::StationAccessPoint(workload) => workload.direction == Direction::Rx,
            _ => false,
        }
    }
}

impl ScenarioFamily for WifiScenario {
    fn validate(&self) -> Result<()> {
        if !self.image.is_wifi() {
            return Err(format!("{} is not a Wi-Fi image", self.image.id()).into());
        }
        if let Some(link) = self.workload.link() {
            self.validate_link(link)?;
        }
        self.validate_datapath()?;
        let image = self.image;
        match &self.workload {
            WifiWorkload::StationUdp(workload) => workload.validate(image)?,
            WifiWorkload::StationTcp(workload) => workload.validate(image)?,
            WifiWorkload::StationIcmp(workload) => workload.validate(image)?,
            WifiWorkload::StationSleep(workload) => workload.validate()?,
            WifiWorkload::StationReconnect(workload) => workload.validate(image)?,
            WifiWorkload::StationApLoss {
                timeout_seconds, ..
            }
            | WifiWorkload::StationApAbsence {
                timeout_seconds, ..
            } => bounded(*timeout_seconds, 30, 300, "timeout_seconds")?,
            WifiWorkload::Role {
                timeout_seconds,
                operation,
                ..
            } => {
                bounded(*timeout_seconds, 10, 180, "timeout_seconds")?;
                operation.validate()?;
            }
            WifiWorkload::MonitorCapture {
                timeout_seconds,
                duration_seconds,
                channel,
                snapshot_length,
                ..
            } => {
                bounded(*timeout_seconds, 10, 180, "timeout_seconds")?;
                bounded(*duration_seconds, 1, 30, "duration_seconds")?;
                if let Some(channel) = channel {
                    bounded(*channel, 1, 13, "channel")?;
                }
                bounded(*snapshot_length, 0, 2304, "snapshot_length")?;
            }
            WifiWorkload::AccessPoint(workload) => workload.validate(image)?,
            WifiWorkload::StationAccessPoint(workload) => workload.validate(image)?,
            WifiWorkload::StationAccessPointReconnect {
                timeout_seconds, ..
            } => bounded(*timeout_seconds, 30, 180, "timeout_seconds")?,
        }
        if image == ImageClass::DiagnosticRxOwnership
            && !matches!(self.workload, WifiWorkload::StationTcp(_))
        {
            return Err(
                "RX ownership observation requires a TCP workload with a bounded measurement window"
                    .into(),
            );
        }
        if image == ImageClass::Performance
            && !matches!(
                self.workload,
                WifiWorkload::StationUdp(_)
                    | WifiWorkload::StationTcp(_)
                    | WifiWorkload::StationIcmp(_)
                    | WifiWorkload::AccessPoint(_)
                    | WifiWorkload::StationAccessPoint(_)
            )
        {
            return Err(
                "performance images admit only externally measured network workloads".into(),
            );
        }
        Ok(())
    }

    fn plan(&self) -> Plan {
        let mut requirements = Requirements {
            // AP and monitor workloads also begin by qualifying the connected
            // STA. Keep that real dependency visible until their lifecycle
            // changes.
            station_network: true,
            ..Requirements::default()
        };
        let mut checks = Vec::new();
        let mut ap_scheduler = WifiApScheduler::Disabled;
        match &self.workload {
            WifiWorkload::StationUdp(workload) => {
                requirements.station_udp_rx_capture = workload.offer.rx_bps.is_some();
                requirements.station_udp_tx_capture = workload.offer.tx_bps.is_some();
                requirements.openwrt_tx_monitor = workload.observation.openwrt_tx_monitor;
                requirements.laptop_air_monitor = workload.observation.independent_air_monitor;
                if let Some(induced) = workload.induced_protection {
                    requirements.air_observer = true;
                    match induced.peer {
                        station::ProtectionPeer::NonHtMember => {
                            requirements.non_ht_member = true;
                        }
                        station::ProtectionPeer::OverlappingLegacyBss => {
                            requirements.legacy_bss = true;
                        }
                    }
                }
                workload.checks(&mut checks);
            }
            WifiWorkload::AccessPoint(workload) => {
                requirements.probe_load = workload.probe_load;
                requirements.laptop_client = workload.clients.laptop();
                requirements.openwrt_client = workload.clients.openwrt();
                requirements.openwrt_tx_monitor = workload.probe_load;
                requirements.laptop_air_monitor = workload.independent_air_monitor;
                requirements.air_observer = workload.protection.is_some();
                if workload.protection.is_some() {
                    checks.extend(station::PROTECTION_CHECKS);
                }
                ap_scheduler = workload.scheduler;
            }
            WifiWorkload::StationAccessPoint(workload) => {
                requirements.laptop_client = true;
                requirements.laptop_air_monitor = workload.independent_air_monitor;
            }
            WifiWorkload::StationAccessPointReconnect { .. } => {
                requirements.laptop_client = true;
                requirements.station_control = true;
            }
            WifiWorkload::StationApLoss {
                require_recovery_echo,
                ..
            } => {
                requirements.station_control = true;
                checks.extend([
                    "wifi.station.ap-loss-reconnected",
                    "wifi.station.control-responsive",
                ]);
                if *require_recovery_echo {
                    checks.push("wifi.station.recovered-ip-exchange");
                }
            }
            WifiWorkload::StationApAbsence {
                initially_absent, ..
            } => {
                requirements.station_control = true;
                checks.push(if *initially_absent {
                    "wifi.station.initial-retry-exhausted"
                } else {
                    "wifi.station.recovery-retry-exhausted"
                });
                checks.push("wifi.station.control-responsive");
            }
            WifiWorkload::StationSleep(_) => checks.push("wifi.station.rf-sleep-observed"),
            WifiWorkload::StationTcp(_)
            | WifiWorkload::StationIcmp(_)
            | WifiWorkload::StationReconnect(_)
            | WifiWorkload::Role { .. }
            | WifiWorkload::MonitorCapture { .. } => {}
        }
        checks.sort_unstable();
        let datapath = self.datapath;
        Plan {
            image: self.image,
            requirements,
            settings: TargetSettings {
                ap_scheduler,
                data_plane: datapath.placement,
                rx_checksum: datapath.rx_checksum,
                tx_udp_checksum: datapath.tx_udp_checksum,
                tx_buffer: datapath.tx_buffer,
                rx_continuation: datapath.rx_continuation,
                l1_cache_counters: datapath.l1_cache_counters,
                station_power_save: match &self.workload {
                    WifiWorkload::StationSleep(workload) => workload.power_save,
                    _ => oer_hil_protocol::wifi::WifiStationPowerSave::None,
                },
            },
            checks,
            wifi: WifiLabUse {
                link: self.workload.link().map(|link| link.phy),
                management_frame_protection: self
                    .workload
                    .link()
                    .map(|link| link.management_frame_protection)
                    .unwrap_or_default(),
                access_point_security: self
                    .workload
                    .link()
                    .map(|link| link.access_point_security)
                    .unwrap_or_default(),
                access_point: matches!(self.workload, WifiWorkload::AccessPoint(_)),
                access_point_beacon: match &self.workload {
                    WifiWorkload::StationSleep(workload) => Some(workload.access_point_beacon),
                    _ => None,
                },
            },
        }
    }

    /// The channel of the scenario's link, as the laboratory sets it up.
    fn air_use(&self) -> Vec<AirUse> {
        vec![AirUse::WifiLink]
    }
}

impl WifiScenario {
    fn validate_link(&self, link: LinkExpectation) -> Result<()> {
        if (link.management_frame_protection.negotiated()
            || link.access_point_security.offers_sae())
            && !matches!(self.workload, WifiWorkload::Role { .. })
        {
            return Err(format!(
                "management_frame_protection={} and access_point_security={} are verified only by the role workload",
                link.management_frame_protection.id(),
                link.access_point_security.id()
            )
            .into());
        }
        if let Some(minimum_mcs) = link.minimum_mcs {
            if !self.image.requires_driver_observation() {
                return Err("minimum_mcs requires a driver-observation image".into());
            }
            let maximum_mcs = match link.phy {
                PhyExpectation::Legacy => {
                    return Err("minimum_mcs requires an HT or HE link".into());
                }
                PhyExpectation::Ht20 | PhyExpectation::Ht40 => 7,
                PhyExpectation::He20 => 9,
            };
            if minimum_mcs > maximum_mcs {
                return Err(format!(
                    "minimum_mcs={minimum_mcs} exceeds the {} capability MCS{maximum_mcs}",
                    link.phy.id(),
                )
                .into());
            }
        }
        if link.guard_interval != HtGuardIntervalExpectation::Any {
            if link.phy == PhyExpectation::He20 {
                return Err(format!(
                    "guard_interval={} is valid only for an HT link",
                    link.guard_interval.id()
                )
                .into());
            }
            if !self.image.requires_driver_observation() {
                return Err(
                    "strict guard_interval requires complete target-side driver observation".into(),
                );
            }
        }
        Ok(())
    }

    fn validate_datapath(&self) -> Result<()> {
        let Datapath {
            placement: _,
            rx_checksum,
            tx_udp_checksum,
            tx_buffer,
            rx_continuation,
            l1_cache_counters,
        } = self.datapath;
        let image = self.image;
        let station_udp = match &self.workload {
            WifiWorkload::StationUdp(workload) => Some(workload.offer),
            _ => None,
        };
        let access_point_udp_tx = match &self.workload {
            WifiWorkload::AccessPoint(workload) => workload
                .traffic
                .udp_offer()
                .is_some_and(Offer::transmit_only),
            _ => false,
        };
        if rx_checksum == WifiRxChecksumPolicy::AssumeValidDiagnostic
            && (image != ImageClass::DiagnosticTaskPoll
                || !station_udp.is_some_and(Offer::receive_only))
        {
            return Err("assume-valid-diagnostic RX checksum policy is restricted to the station UDP RX task-poll diagnostic".into());
        }
        if tx_udp_checksum == WifiTxUdpChecksumPolicy::OmitIpv4Diagnostic
            && (!matches!(
                image,
                ImageClass::DiagnosticTaskPoll | ImageClass::DiagnosticCore0RxCoarse
            ) || !(station_udp.is_some_and(Offer::transmit_only)
                || matches!(&self.workload, WifiWorkload::AccessPoint(workload)
                    if matches!(&workload.traffic, AccessPointTraffic::UdpMultiClient(traffic)
                        if traffic.offer.transmit_only()))))
        {
            return Err("omit-ipv4-diagnostic TX UDP checksum policy is restricted to a UDP TX task-poll or coarse-cycle diagnostic".into());
        }
        if tx_buffer != WifiTxBufferPolicy::OwnedSramPromotion
            && (!matches!(
                image,
                ImageClass::DiagnosticTaskResidence
                    | ImageClass::DiagnosticTxArchitecture
                    | ImageClass::DiagnosticTaskPoll
                    | ImageClass::DiagnosticCore0RxCoarse
            ) || !access_point_udp_tx)
        {
            return Err(
                "TX buffer policies are restricted to a compatible AP UDP TX diagnostic image"
                    .into(),
            );
        }
        if rx_continuation != WifiRxContinuationPolicy::ImmediateSoftwareProbe
            && (!image.is_core0_rx_cycle_diagnostic() || !self.workload.receive_only_udp())
        {
            return Err(
                "selectable RX continuation is restricted to a Core0 UDP RX diagnostic".into(),
            );
        }
        let coarse_ap_tx = image == ImageClass::DiagnosticCore0RxCoarse
            && matches!(&self.workload, WifiWorkload::AccessPoint(workload)
                if matches!(&workload.traffic, AccessPointTraffic::Udp(traffic)
                    if traffic.offer.transmit_only()));
        if l1_cache_counters && image != ImageClass::DiagnosticCore0RxCycles && !coarse_ap_tx {
            return Err("L1 cache counters require the Core0 RX cycle image or an AP UDP TX coarse-cycle diagnostic".into());
        }
        Ok(())
    }
}

impl Workload for WifiScenario {
    fn run(&self, output: &Path, context: &Context<'_>, fixtures: &Fixtures) -> Result<()> {
        run::execute(self, output, context, fixtures.require::<Prepared>()?)
    }
}

#[cfg(test)]
mod tests;
