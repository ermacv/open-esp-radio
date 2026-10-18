//! Access-point workloads: the target runs the AP for laboratory clients.

use oer_hil_protocol::{wifi::WifiAccessPointSecurity, wifi::WifiApScheduler};
use oer_hil_scenario_catalog::bounded;
use oer_hil_scenario_catalog::link::{HtGuardIntervalExpectation, PhyExpectation};
use oer_hil_schema::image::ImageClass;
use serde::{Deserialize, Serialize};

use super::{
    Direction, LinkExpectation, Offer, RateFloors,
    station::{driver_observed_criteria, validate_icmp},
};
use crate::Result;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessPoint {
    /// The client link; it also selects the target AP channel width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<LinkExpectation>,
    pub cycles: u8,
    pub boots: u8,
    pub timeout_seconds: u16,
    #[serde(default = "default_security")]
    pub security: WifiAccessPointSecurity,
    #[serde(default)]
    pub scheduler: WifiApScheduler,
    #[serde(default)]
    pub clients: AccessPointClients,
    /// Background probe requests from the OpenWrt monitor during traffic.
    #[serde(default)]
    pub probe_load: bool,
    /// Capture the channel through the laptop's independent adapter.
    #[serde(default)]
    pub independent_air_monitor: bool,
    /// Observe, with the independent observer, how the AP protects its HT
    /// data to the OpenWrt client while the laptop is a non-HT member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protection: Option<AccessPointProtection>,
    pub traffic: AccessPointTraffic,
}

const fn default_security() -> WifiAccessPointSecurity {
    WifiAccessPointSecurity::Wpa2Personal
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessPointProtection {
    /// Minimum share of the AP's observed HT data PPDUs to the OpenWrt
    /// client that an RTS/CTS exchange precedes.
    pub minimum_protected_ppdu_percent: u8,
}

/// The physical clients that associate with the target AP.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AccessPointClients {
    Laptop {
        #[serde(default)]
        laptop_phy: LaptopPhy,
    },
    /// The controlled OpenWrt client, with optional diagnostic transmit
    /// mutations owned by its scoped client interface. Deleting that
    /// interface restores automatic rate control.
    #[serde(rename = "openwrt")]
    OpenWrt {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fixed_ht_mcs: Option<u8>,
        /// Fix the client's transmit guard interval to the link's strict
        /// expectation.
        #[serde(default)]
        fixed_guard_interval: bool,
    },
    /// The laptop as primary client and the OpenWrt client concurrently.
    #[serde(rename = "laptop-and-openwrt")]
    LaptopAndOpenWrt {
        #[serde(default)]
        laptop_phy: LaptopPhy,
    },
}

/// The capabilities the laptop client advertises.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LaptopPhy {
    #[default]
    Ht,
    /// A Clause 18 ERP-OFDM station: the AP must protect HT PPDUs.
    NonHt,
}

impl Default for AccessPointClients {
    fn default() -> Self {
        Self::Laptop {
            laptop_phy: LaptopPhy::Ht,
        }
    }
}

impl AccessPointClients {
    pub const fn laptop(self) -> bool {
        self.laptop_phy().is_some()
    }

    /// The capabilities the laptop advertises, when it is a client.
    pub const fn laptop_phy(self) -> Option<LaptopPhy> {
        match self {
            Self::Laptop { laptop_phy } | Self::LaptopAndOpenWrt { laptop_phy } => Some(laptop_phy),
            Self::OpenWrt { .. } => None,
        }
    }

    pub const fn openwrt(self) -> bool {
        matches!(self, Self::OpenWrt { .. } | Self::LaptopAndOpenWrt { .. })
    }

    pub const fn count(self) -> u8 {
        match self {
            Self::Laptop { .. } | Self::OpenWrt { .. } => 1,
            Self::LaptopAndOpenWrt { .. } => 2,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AccessPointTraffic {
    None {},
    Icmp(AccessPointIcmp),
    Udp(AccessPointUdp),
    UdpMultiClient(AccessPointUdpMultiClient),
    Tcp(AccessPointTcp),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessPointIcmp {
    pub count: u16,
    pub interval_ms: u16,
    pub timeout_ms: u16,
    pub payload_bytes: u16,
    #[serde(default)]
    pub maximum_lost: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_p95_ms: Option<u16>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessPointUdp {
    pub duration_seconds: u16,
    pub payload_bytes: u16,
    pub offer: Offer,
    #[serde(default)]
    pub criteria: UdpCriteria,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct UdpCriteria {
    pub exact_delivery: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_rx_bps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_tx_bps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_combined_bps: Option<u64>,
}

impl UdpCriteria {
    pub fn floors(self) -> RateFloors {
        RateFloors {
            minimum_rx_bps: self.minimum_rx_bps,
            minimum_tx_bps: self.minimum_tx_bps,
            minimum_combined_bps: self.minimum_combined_bps,
        }
    }
}

/// Equal per-flow offers to the laptop and OpenWrt clients, with optional
/// distinct offers for the second (OpenWrt) client.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessPointUdpMultiClient {
    pub duration_seconds: u16,
    pub payload_bytes: u16,
    /// The offer to each client flow.
    pub offer: Offer,
    #[serde(default)]
    pub secondary: SecondaryOffer,
    pub criteria: MultiClientCriteria,
}

/// The second client's offer where it differs from the per-flow offer.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecondaryOffer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rx_bps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_bps: Option<u64>,
    /// Target-TX pacing group of the second flow: a sparse burst described
    /// independently of its average offered rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_pacing_group_datagrams: Option<u8>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MultiClientCriteria {
    #[serde(default)]
    pub exact_delivery: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_rx_bps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_tx_bps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_combined_bps: Option<u64>,
    pub minimum_bps_per_flow: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_flow_skew_percent: Option<u8>,
    /// Minimum fraction of each configured RX offer accepted by host UDP
    /// send calls. Independent of target delivery criteria.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_host_offer_percent: Option<u8>,
    /// Host-observed upper bound between consecutive datagrams of the second
    /// target-TX flow, for sparse-peer service; not an air-latency estimate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_secondary_tx_interarrival_ms: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_secondary_tx_datagrams: Option<u16>,
}

impl MultiClientCriteria {
    pub fn floors(self) -> RateFloors {
        RateFloors {
            minimum_rx_bps: self.minimum_rx_bps,
            minimum_tx_bps: self.minimum_tx_bps,
            minimum_combined_bps: self.minimum_combined_bps,
        }
    }
}

impl AccessPointUdpMultiClient {
    /// The summed offer of both client flows.
    pub fn total_offer(&self) -> Offer {
        let sum = |primary: Option<u64>, secondary: Option<u64>| {
            primary.map(|primary| primary.saturating_add(secondary.unwrap_or(primary)))
        };
        Offer {
            rx_bps: sum(self.offer.rx_bps, self.secondary.rx_bps),
            tx_bps: sum(self.offer.tx_bps, self.secondary.tx_bps),
        }
    }

    fn validate(&self) -> Result<()> {
        bounded(self.duration_seconds, 5, 300, "traffic.duration_seconds")?;
        bounded(self.payload_bytes, 64, 1472, "traffic.payload_bytes")?;
        self.offer.validate()?;
        let direction = self.offer.direction();
        let secondary = self.secondary;
        if (secondary.rx_bps.is_some() && !direction.receives())
            || (secondary.tx_bps.is_some() && !direction.transmits())
        {
            return Err("secondary offered rates do not match the multi-client direction".into());
        }
        for (field, rate) in [
            ("traffic.secondary.rx_bps", secondary.rx_bps),
            ("traffic.secondary.tx_bps", secondary.tx_bps),
        ] {
            if let Some(rate) = rate {
                bounded(rate, 1_000, 1_000_000_000, field)?;
            }
        }
        if let Some(group) = secondary.tx_pacing_group_datagrams {
            bounded(group, 1, 64, "traffic.secondary.tx_pacing_group_datagrams")?;
            if !direction.transmits() {
                return Err("secondary TX pacing requires a target-TX data plane".into());
            }
        }
        let criteria = self.criteria;
        criteria.floors().validate(self.total_offer())?;
        if let Some(minimum) = criteria.minimum_host_offer_percent {
            if !direction.receives() {
                return Err("minimum_host_offer_percent requires an RX offer".into());
            }
            bounded(minimum, 1, 100, "minimum_host_offer_percent")?;
        }
        let per_flow_offer = [
            self.offer.rx_bps,
            secondary.rx_bps.or(self.offer.rx_bps),
            self.offer.tx_bps,
            secondary.tx_bps.or(self.offer.tx_bps),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(0);
        bounded(
            criteria.minimum_bps_per_flow,
            1,
            per_flow_offer,
            "minimum_bps_per_flow",
        )?;
        if let Some(maximum) = criteria.maximum_flow_skew_percent {
            bounded(maximum, 1, 100, "maximum_flow_skew_percent")?;
            if secondary
                .rx_bps
                .is_some_and(|rate| Some(rate) != self.offer.rx_bps)
                || secondary
                    .tx_bps
                    .is_some_and(|rate| Some(rate) != self.offer.tx_bps)
            {
                return Err(
                    "maximum_flow_skew_percent is invalid for unequal offered rates".into(),
                );
            }
        }
        if let Some(maximum) = criteria.maximum_secondary_tx_interarrival_ms {
            if maximum == 0 {
                return Err("maximum_secondary_tx_interarrival_ms must be nonzero".into());
            }
            if secondary.tx_bps.is_none() || secondary.tx_pacing_group_datagrams.is_none() {
                return Err("maximum_secondary_tx_interarrival_ms requires an explicitly paced secondary TX flow".into());
            }
        }
        if let Some(minimum) = criteria.minimum_secondary_tx_datagrams {
            if minimum < 2 {
                return Err(
                    "minimum_secondary_tx_datagrams must cover at least one inter-arrival".into(),
                );
            }
            if secondary.tx_bps.is_none() {
                return Err(
                    "minimum_secondary_tx_datagrams requires an explicit secondary TX flow".into(),
                );
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessPointTcp {
    pub duration_seconds: u16,
    pub chunk_bytes: u16,
    pub offer: Offer,
    #[serde(default)]
    pub criteria: TcpCriteria,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TcpCriteria {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_rx_bps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_tx_bps: Option<u64>,
}

impl TcpCriteria {
    pub fn floors(self) -> RateFloors {
        RateFloors {
            minimum_rx_bps: self.minimum_rx_bps,
            minimum_tx_bps: self.minimum_tx_bps,
            minimum_combined_bps: None,
        }
    }
}

impl AccessPointTraffic {
    /// The offer of a UDP data plane: per flow for multi-client traffic.
    pub fn udp_offer(&self) -> Option<Offer> {
        match self {
            Self::Udp(traffic) => Some(traffic.offer),
            Self::UdpMultiClient(traffic) => Some(traffic.offer),
            Self::None {} | Self::Icmp(_) | Self::Tcp(_) => None,
        }
    }

    /// The direction of the target's data plane; ICMP exercises both.
    pub fn direction(&self) -> Option<Direction> {
        match self {
            Self::None {} => None,
            Self::Icmp(_) => Some(Direction::Bidirectional),
            Self::Udp(traffic) => Some(traffic.offer.direction()),
            Self::UdpMultiClient(traffic) => Some(traffic.offer.direction()),
            Self::Tcp(traffic) => Some(traffic.offer.direction()),
        }
    }

    fn validate(&self, image: ImageClass) -> Result<()> {
        match self {
            Self::None {} => Ok(()),
            Self::Icmp(traffic) => validate_icmp(
                traffic.count,
                traffic.interval_ms,
                traffic.timeout_ms,
                traffic.payload_bytes,
            ),
            Self::Udp(traffic) => {
                bounded(traffic.duration_seconds, 5, 300, "traffic.duration_seconds")?;
                bounded(traffic.payload_bytes, 64, 1472, "traffic.payload_bytes")?;
                traffic.offer.validate()?;
                traffic.criteria.floors().validate(traffic.offer)?;
                driver_observed_criteria(image, traffic.criteria.exact_delivery, false)
            }
            Self::UdpMultiClient(traffic) => {
                traffic.validate()?;
                driver_observed_criteria(image, traffic.criteria.exact_delivery, false)
            }
            Self::Tcp(traffic) => {
                bounded(traffic.duration_seconds, 5, 300, "traffic.duration_seconds")?;
                bounded(traffic.chunk_bytes, 64, 32_768, "traffic.chunk_bytes")?;
                traffic.offer.validate()?;
                traffic.criteria.floors().validate(traffic.offer)
            }
        }
    }
}

impl AccessPoint {
    pub(super) fn validate(&self, image: ImageClass) -> Result<()> {
        bounded(self.cycles, 1, 8, "cycles")?;
        bounded(self.boots, 1, 20, "boots")?;
        bounded(self.timeout_seconds, 20, 180, "timeout_seconds")?;
        self.traffic.validate(image)?;
        if self
            .link
            .is_some_and(|link| link.phy == PhyExpectation::He20)
        {
            return Err("the target AP qualifies HT20 or HT40 client links".into());
        }
        if self.security == WifiAccessPointSecurity::Open
            && !matches!(self.clients, AccessPointClients::OpenWrt { .. })
        {
            return Err("open AP qualification requires the controlled OpenWrt client".into());
        }
        match self.clients {
            AccessPointClients::OpenWrt {
                fixed_ht_mcs,
                fixed_guard_interval,
            } => {
                if !matches!(
                    self.traffic,
                    AccessPointTraffic::Udp(_) | AccessPointTraffic::Icmp(_)
                ) {
                    return Err(
                        "the OpenWrt primary AP client supports UDP and ICMP workloads".into(),
                    );
                }
                let mutated = fixed_ht_mcs.is_some() || fixed_guard_interval;
                if let Some(mcs) = fixed_ht_mcs {
                    bounded(mcs, 0, 7, "clients.fixed_ht_mcs")?;
                }
                if mutated
                    && (self
                        .link
                        .is_none_or(|link| link.phy != PhyExpectation::Ht40)
                        || !image.requires_driver_observation()
                        || !self.independent_air_monitor)
                {
                    return Err("OpenWrt client mutations require an HT40 link with driver and independent-air evidence".into());
                }
                if fixed_guard_interval
                    && self
                        .link
                        .is_none_or(|link| link.guard_interval == HtGuardIntervalExpectation::Any)
                {
                    return Err(
                        "a fixed client guard interval requires a strict guard-interval expectation"
                            .into(),
                    );
                }
            }
            AccessPointClients::Laptop { .. } | AccessPointClients::LaptopAndOpenWrt { .. } => {}
        }
        if matches!(self.traffic, AccessPointTraffic::UdpMultiClient(_))
            && !matches!(self.clients, AccessPointClients::LaptopAndOpenWrt { .. })
        {
            return Err("multi-client AP UDP requires the laptop and OpenWrt clients".into());
        }
        if self.independent_air_monitor
            && !(matches!(self.clients, AccessPointClients::OpenWrt { .. })
                && matches!(self.traffic, AccessPointTraffic::Udp(_)))
        {
            return Err(
                "independent AP air evidence requires UDP with the OpenWrt primary client".into(),
            );
        }
        if self.probe_load
            && (image != ImageClass::DiagnosticTaskPoll
                || !matches!(&self.traffic, AccessPointTraffic::UdpMultiClient(traffic)
                    if traffic.offer.transmit_only() && traffic.duration_seconds == 12))
        {
            return Err(
                "probe load requires diagnostic-task-poll and 12-second multi-client UDP TX".into(),
            );
        }
        if let Some(protection) = self.protection {
            bounded(
                protection.minimum_protected_ppdu_percent,
                1,
                100,
                "protection.minimum_protected_ppdu_percent",
            )?;
            if self.clients
                != (AccessPointClients::LaptopAndOpenWrt {
                    laptop_phy: LaptopPhy::NonHt,
                })
                || self
                    .link
                    .is_none_or(|link| link.phy == PhyExpectation::He20)
                || self.independent_air_monitor
                || !matches!(&self.traffic, AccessPointTraffic::UdpMultiClient(traffic)
                    if traffic.offer.direction().transmits())
            {
                return Err("AP protection needs a non-HT laptop, the OpenWrt client, an HT link and multi-client UDP TX".into());
            }
        }
        if self.scheduler != WifiApScheduler::Disabled && self.link.is_none() {
            return Err(
                "AP scheduler comparison requires an explicit HT20/HT40 client link".into(),
            );
        }
        if let Some(link) = self.link
            && link.minimum_mcs.is_some()
        {
            if link.phy != PhyExpectation::Ht40 || link.minimum_mcs != Some(7) {
                return Err("AP target MCS evidence supports only HT40 MCS7".into());
            }
            if matches!(self.traffic, AccessPointTraffic::None {}) {
                return Err("AP minimum_mcs requires a data-plane workload".into());
            }
        }
        Ok(())
    }
}
