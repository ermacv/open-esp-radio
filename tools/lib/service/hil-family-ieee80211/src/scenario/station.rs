//! Station workloads: the target joins the laboratory AP.

use oer_hil_scenario_catalog::bounded;
use oer_hil_scenario_catalog::link::{HtGuardIntervalExpectation, PhyExpectation};
use oer_hil_schema::image::ImageClass;
use serde::{Deserialize, Serialize};

use super::{CORE0_RX_CYCLE_MAX_DURATION_SECONDS, Direction, LinkExpectation, Offer, RateFloors};
use crate::Result;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StationUdp {
    pub link: LinkExpectation,
    pub duration_seconds: u16,
    pub payload_bytes: u16,
    pub offer: Offer,
    /// Fix the OpenWrt AP's transmit guard interval to the link's strict
    /// expectation. A diagnostic fixture mutation, not an expectation.
    #[serde(default)]
    pub fixed_fixture_guard_interval: bool,
    #[serde(default)]
    pub observation: AirObservation,
    /// A real laboratory peer that makes the station fixture AP advertise
    /// BSS protection for the whole workload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub induced_protection: Option<InducedProtection>,
    /// Suspend the shared PHY's periodic tracking for the traffic session,
    /// the arm without tracking of a PHY maintenance experiment.
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub suspend_phy_tracking: bool,
    /// Time this many forced shared-PHY tracking ticks of each branch set
    /// during the traffic and again on the idle link after it (). It
    /// needs the `diagnostic-phy-tracking-hold` image and a receiving offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phy_tracking_hold: Option<u16>,
    /// Take this many samples of a software interrupt's latency at the radio
    /// interrupts' level during the traffic and again on the idle link
    /// (). It needs the `diagnostic-interrupt-latency` image and a
    /// receiving offer, and excludes `phy_tracking_hold`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt_latency_probe: Option<u16>,
    #[serde(default)]
    pub criteria: StationUdpCriteria,
}

/// Protection the station fixture AP must advertise during the workload,
/// and how completely the target must follow it on the air.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InducedProtection {
    pub peer: ProtectionPeer,
    /// Minimum share of the target's observed data PPDUs that an RTS/CTS
    /// exchange precedes. The remainder bounds observer capture loss.
    pub minimum_protected_ppdu_percent: u8,
}

/// A standard-conformant peer that obliges the station fixture AP to
/// advertise protection. Both use the laptop radio.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProtectionPeer {
    /// The laptop joins the AP without HT capability: HT Protection becomes
    /// non-HT mixed.
    NonHtMember,
    /// The laptop hosts an 802.11b BSS on the AP's channel: the AP observes
    /// an overlapping legacy BSS and sets ERP Use_Protection.
    OverlappingLegacyBss,
}

/// Independent air observers of the station link.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AirObservation {
    /// Capture the OpenWrt AP's own TX monitor tap. Diagnostic only.
    pub openwrt_tx_monitor: bool,
    /// Capture the channel through the laptop's independent adapter,
    /// correlated with the OpenWrt tap.
    pub independent_air_monitor: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct StationUdpCriteria {
    pub exact_delivery: bool,
    pub require_no_beacon_loss: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_rx_bps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_tx_bps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_combined_bps: Option<u64>,
    /// Maximum silence in the complete station UDP RX observation window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_rx_silence_ms: Option<u32>,
    /// Maximum pre-workload channel utilization reported by the AP, in the
    /// native 0..=255 BSS-load scale: a ceiling-scenario precondition, not
    /// an inferred throughput failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_idle_channel_utilization_255: Option<u8>,
}

impl StationUdpCriteria {
    pub fn floors(self) -> RateFloors {
        RateFloors {
            minimum_rx_bps: self.minimum_rx_bps,
            minimum_tx_bps: self.minimum_tx_bps,
            minimum_combined_bps: self.minimum_combined_bps,
        }
    }
}

impl StationUdp {
    pub(super) fn validate(&self, image: ImageClass) -> Result<()> {
        bounded(self.duration_seconds, 5, 300, "duration_seconds")?;
        bounded(self.payload_bytes, 64, 1472, "payload_bytes")?;
        self.offer.validate()?;
        if image.is_core0_rx_cycle_diagnostic()
            && self.duration_seconds > CORE0_RX_CYCLE_MAX_DURATION_SECONDS
        {
            return Err(format!(
                "Core0 cycle diagnostic duration {} exceeds the u32-safe {CORE0_RX_CYCLE_MAX_DURATION_SECONDS} second interval",
                self.duration_seconds
            )
            .into());
        }
        let direction = self.offer.direction();
        if let Some(ticks) = self.phy_tracking_hold {
            bounded(ticks, 1, 10, "phy_tracking_hold")?;
            if image != ImageClass::DiagnosticPhyTrackingHold {
                return Err(
                    "phy_tracking_hold requires the diagnostic-phy-tracking-hold image".into(),
                );
            }
            let window = oer_hil_workload::phy_tracking_hold::window(ticks);
            if u64::from(self.duration_seconds) < window.as_secs() {
                return Err(format!(
                    "phy_tracking_hold of {ticks} ticks needs at least {} s of traffic",
                    window.as_secs()
                )
                .into());
            }
            if direction != Direction::Rx || self.suspend_phy_tracking {
                return Err(
                    "phy_tracking_hold runs beside a receive-only offer, with tracking not suspended by the scenario"
                        .into(),
                );
            }
        }
        if let Some(samples) = self.interrupt_latency_probe {
            bounded(samples, 1, 1024, "interrupt_latency_probe")?;
            if image != ImageClass::DiagnosticInterruptLatency {
                return Err(
                    "interrupt_latency_probe requires the diagnostic-interrupt-latency image"
                        .into(),
                );
            }
            if direction != Direction::Rx || self.phy_tracking_hold.is_some() {
                return Err(
                    "interrupt_latency_probe runs beside a receive-only offer and excludes phy_tracking_hold"
                        .into(),
                );
            }
        }
        let criteria = self.criteria;
        criteria.floors().validate(self.offer)?;
        driver_observed_criteria(
            image,
            criteria.exact_delivery,
            criteria.require_no_beacon_loss,
        )?;
        if let Some(maximum) = criteria.maximum_rx_silence_ms
            && (maximum == 0 || !direction.receives())
        {
            return Err(
                "maximum_rx_silence_ms requires a nonzero bound on a receiving offer".into(),
            );
        }
        if let Some(maximum) = criteria.maximum_idle_channel_utilization_255
            && (maximum == 0 || direction == Direction::Bidirectional)
        {
            return Err(
                "maximum_idle_channel_utilization_255 requires a nonzero bound on a one-way offer"
                    .into(),
            );
        }
        let observation = self.observation;
        if observation.openwrt_tx_monitor
            && (!direction.receives()
                || !matches!(
                    image,
                    ImageClass::Correctness
                        | ImageClass::DiagnosticTaskResidence
                        | ImageClass::DiagnosticTaskPoll
                        | ImageClass::DiagnosticRxDelivery
                        | ImageClass::DiagnosticCore0RxCoarse
                        | ImageClass::DiagnosticCore0RxCycles
                        | ImageClass::DiagnosticStationExit
                ))
        {
            return Err("OpenWrt TX-monitor evidence requires a receiving offer with a correctness or supported diagnostic image".into());
        }
        if observation.independent_air_monitor && !observation.openwrt_tx_monitor {
            return Err(
                "independent station air evidence requires the correlated OpenWrt TX monitor"
                    .into(),
            );
        }
        if self.fixed_fixture_guard_interval
            && (self.link.phy == PhyExpectation::He20
                || self.link.guard_interval == HtGuardIntervalExpectation::Any
                || !observation.openwrt_tx_monitor
                || !observation.independent_air_monitor)
        {
            return Err("a fixed OpenWrt guard interval requires a strict HT guard-interval expectation and both air observers".into());
        }
        if let Some(induced) = self.induced_protection {
            if observation.independent_air_monitor {
                return Err(
                    "induced protection uses the laptop radio, which cannot also observe the air"
                        .into(),
                );
            }
            if !direction.transmits() || self.link.phy == PhyExpectation::He20 {
                return Err(
                    "induced protection is observed on the target's HT data transmissions".into(),
                );
            }
            bounded(
                induced.minimum_protected_ppdu_percent,
                1,
                100,
                "induced_protection.minimum_protected_ppdu_percent",
            )?;
        }
        if direction == Direction::Tx && self.link.phy == PhyExpectation::Ht20 {
            return Err("station UDP TX requires HE20 or HT40".into());
        }
        if direction == Direction::Bidirectional && self.link.phy == PhyExpectation::Ht20 {
            return Err("bidirectional station UDP requires HE20 or HT40".into());
        }
        Ok(())
    }

    pub(super) fn checks(&self, checks: &mut Vec<&'static str>) {
        if self.induced_protection.is_some() {
            checks.extend(PROTECTION_CHECKS);
        }
        if !self.offer.receive_only() {
            return;
        }
        if self.criteria.minimum_rx_bps.is_some() {
            checks.extend(["udp.rx.target-rate", "udp.rx.host-offer-rate"]);
        }
        if self.criteria.maximum_rx_silence_ms.is_some() {
            checks.push("udp.rx.maximum-silence");
        }
    }
}

/// Named observations of the target following induced BSS protection.
pub const PROTECTION_CHECKS: [&str; 3] = [
    "wifi.protection.rts-cts-before-data",
    "wifi.protection.control-rate",
    "wifi.protection.nav-covers-exchange",
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StationTcp {
    pub link: LinkExpectation,
    pub duration_seconds: u16,
    pub chunk_bytes: u16,
    pub offer: Offer,
    #[serde(default)]
    pub criteria: StationTcpCriteria,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct StationTcpCriteria {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_rx_bps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_tx_bps: Option<u64>,
    pub require_no_beacon_loss: bool,
}

impl StationTcp {
    pub(super) fn validate(&self, image: ImageClass) -> Result<()> {
        bounded(self.duration_seconds, 5, 300, "duration_seconds")?;
        bounded(self.chunk_bytes, 64, 32_768, "chunk_bytes")?;
        self.offer.validate()?;
        RateFloors {
            minimum_rx_bps: self.criteria.minimum_rx_bps,
            minimum_tx_bps: self.criteria.minimum_tx_bps,
            minimum_combined_bps: None,
        }
        .validate(self.offer)?;
        driver_observed_criteria(image, false, self.criteria.require_no_beacon_loss)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StationIcmp {
    pub link: LinkExpectation,
    pub count: u16,
    pub interval_ms: u16,
    pub timeout_ms: u16,
    pub payload_bytes: u16,
    #[serde(default)]
    pub criteria: IcmpCriteria,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct IcmpCriteria {
    pub maximum_lost: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_p95_ms: Option<u16>,
    pub require_no_beacon_loss: bool,
}

impl StationIcmp {
    pub(super) fn validate(&self, image: ImageClass) -> Result<()> {
        validate_icmp(
            self.count,
            self.interval_ms,
            self.timeout_ms,
            self.payload_bytes,
        )?;
        driver_observed_criteria(image, false, self.criteria.require_no_beacon_loss)
    }
}

/// A station in modem sleep under ICMP probing: the scenario sets the
/// access point's beacon schedule and the station's power save, and the
/// workload records the RF sleep edges' MAC-counter and monotonic readings.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StationSleep {
    pub link: LinkExpectation,
    pub power_save: oer_hil_protocol::wifi::WifiStationPowerSave,
    pub access_point_beacon: oer_hil_scenario_catalog::link::AccessPointBeacon,
    pub count: u16,
    pub interval_ms: u16,
    pub timeout_ms: u16,
    pub payload_bytes: u16,
}

impl StationSleep {
    pub(super) fn validate(&self) -> Result<()> {
        use oer_hil_protocol::wifi::WifiStationPowerSave;
        match self.power_save {
            WifiStationPowerSave::None => {
                return Err("station-sleep requires a modem-sleep power_save".into());
            }
            WifiStationPowerSave::MaxModem { listen_interval } => {
                bounded(listen_interval, 1, u16::MAX, "power_save.listen_interval")?;
            }
            WifiStationPowerSave::MinModem => {}
        }
        self.access_point_beacon.validate()?;
        validate_icmp(
            self.count,
            self.interval_ms,
            self.timeout_ms,
            self.payload_bytes,
        )
    }
}

pub(super) fn validate_icmp(
    count: u16,
    interval_ms: u16,
    timeout_ms: u16,
    payload_bytes: u16,
) -> Result<()> {
    bounded(count, 1, u16::MAX, "count")?;
    bounded(interval_ms, 1, 10_000, "interval_ms")?;
    bounded(timeout_ms, 1, 60_000, "timeout_ms")?;
    bounded(payload_bytes, 0, 1400, "payload_bytes")
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StationReconnect {
    pub link: LinkExpectation,
    pub cycles: u8,
    pub boots: u8,
    pub timeout_seconds: u16,
    #[serde(default)]
    pub criteria: BeaconCriteria,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BeaconCriteria {
    pub require_no_beacon_loss: bool,
}

impl StationReconnect {
    pub(super) fn validate(&self, image: ImageClass) -> Result<()> {
        bounded(self.cycles, 1, 8, "cycles")?;
        bounded(self.boots, 1, 100, "boots")?;
        bounded(self.timeout_seconds, 10, 300, "timeout_seconds")?;
        driver_observed_criteria(image, false, self.criteria.require_no_beacon_loss)
    }
}

/// A Wi-Fi role transition exercised on a connected station.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RoleOperation {
    Stop {},
    Start {},
    Restart {
        #[serde(default = "one_cycle")]
        cycles: u8,
        /// What the restart does to shared RF: Wi-Fi as the last PHY client
        /// closes and wakes it, beside another client RF stays open.
        rf: RestartRf,
    },
    Scan {},
    Monitor {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<u8>,
        #[serde(default = "default_dwell_seconds")]
        dwell_seconds: u8,
        #[serde(default = "default_snapshot_length")]
        snapshot_length: u16,
    },
    AccessPoint {},
    StationAccessPoint {
        #[serde(default = "default_dwell_seconds")]
        dwell_seconds: u8,
    },
    Roundtrip {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<u8>,
        #[serde(default = "default_dwell_seconds")]
        dwell_seconds: u8,
        #[serde(default = "default_snapshot_length")]
        snapshot_length: u16,
    },
}

/// What an idle radio restart does to shared RF.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartRf {
    /// Wi-Fi is the image's only PHY client: its release closes RF and the
    /// rejoin wakes it.
    ClosedAndWoken,
    /// Another client (the Bluetooth LE application) keeps RF open.
    KeptOpen,
}

const fn one_cycle() -> u8 {
    1
}

const fn default_dwell_seconds() -> u8 {
    3
}

const fn default_snapshot_length() -> u16 {
    256
}

impl RoleOperation {
    pub(super) fn validate(self) -> Result<()> {
        match self {
            Self::Restart { cycles, .. } => bounded(cycles, 1, 10, "cycles"),
            Self::Monitor {
                channel,
                dwell_seconds,
                snapshot_length,
            }
            | Self::Roundtrip {
                channel,
                dwell_seconds,
                snapshot_length,
            } => {
                if let Some(channel) = channel {
                    bounded(channel, 1, 13, "channel")?;
                }
                bounded(dwell_seconds, 1, 30, "dwell_seconds")?;
                bounded(snapshot_length, 0, 2304, "snapshot_length")
            }
            Self::StationAccessPoint { dwell_seconds } => {
                bounded(dwell_seconds, 1, 30, "dwell_seconds")
            }
            Self::Stop {} | Self::Start {} | Self::Scan {} | Self::AccessPoint {} => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StationAccessPoint {
    pub link: LinkExpectation,
    pub timeout_seconds: u16,
    pub duration_seconds: u16,
    pub direction: Direction,
    pub rate_bps_per_flow: u64,
    pub minimum_bps_per_flow: u64,
    pub maximum_fairness_skew_percent: u8,
    pub payload_bytes: u16,
    #[serde(default)]
    pub independent_air_monitor: bool,
}

impl StationAccessPoint {
    pub(super) fn validate(&self, image: ImageClass) -> Result<()> {
        bounded(self.timeout_seconds, 30, 180, "timeout_seconds")?;
        bounded(self.duration_seconds, 5, 120, "duration_seconds")?;
        if image.is_core0_rx_cycle_diagnostic()
            && self.duration_seconds > CORE0_RX_CYCLE_MAX_DURATION_SECONDS
        {
            return Err(format!(
                "Core0 cycle diagnostic duration {} exceeds the u32-safe {CORE0_RX_CYCLE_MAX_DURATION_SECONDS} second interval",
                self.duration_seconds
            )
            .into());
        }
        bounded(self.payload_bytes, 64, 1472, "payload_bytes")?;
        bounded(
            self.maximum_fairness_skew_percent,
            1,
            100,
            "maximum_fairness_skew_percent",
        )?;
        bounded(
            self.rate_bps_per_flow,
            100_000,
            100_000_000,
            "rate_bps_per_flow",
        )?;
        bounded(
            self.minimum_bps_per_flow,
            1,
            self.rate_bps_per_flow,
            "minimum_bps_per_flow",
        )
    }
}

/// Exact delivery and beacon continuity are verdicts over the target's own
/// driver observation, which performance images do not collect.
pub(super) fn driver_observed_criteria(
    image: ImageClass,
    exact_delivery: bool,
    require_no_beacon_loss: bool,
) -> Result<()> {
    if require_no_beacon_loss && !image.requires_driver_observation() {
        return Err("this image cannot use the driver-observed beacon-loss verdict".into());
    }
    if exact_delivery && image == ImageClass::Performance {
        return Err(
            "performance images cannot claim exact delivery without driver observation".into(),
        );
    }
    Ok(())
}
