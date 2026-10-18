//! Serialized AP qualification report model.

use serde::Serialize;

use oer_hil_protocol::{network::Ipv4Endpoint, network::SESSION_FLOW_CAPACITY};

use crate::scenario::Direction;
use oer_hil_family_ieee80211_fixture::{
    local::air_monitor::LocalAirMonitorEvidence, openwrt::client::OpenWrtClientFixturePreparation,
    openwrt::client::OpenWrtClientLinkEvidence, openwrt::client::SecondaryClientProbeEvidence,
};

pub(super) const ACCESS_POINT_REPORT_SCHEMA: u8 = 7;

#[derive(Serialize)]
pub(super) struct AccessPointReport {
    pub(super) schema: u8,
    pub(super) fixture_preparation: Option<OpenWrtClientFixturePreparation>,
    pub(super) boots: Vec<BootReport>,
}

#[derive(Serialize)]
pub(super) struct BootReport {
    pub(super) boot: u8,
    pub(super) cycles: Vec<CycleReport>,
    pub(super) error: Option<String>,
}

#[derive(Serialize)]
pub(super) struct CycleReport {
    pub(super) cycle: u8,
    pub(super) traffic: TrafficReport,
    pub(super) secondary_client: Option<SecondaryClientProbeEvidence>,
    pub(super) primary_client_link: Option<super::clients::ClientLinkEvidence>,
    pub(super) secondary_client_link: Option<OpenWrtClientLinkEvidence>,
    pub(super) independent_air: Option<LocalAirMonitorEvidence>,
    /// Intrusive driver-internal evidence is absent from observer-free
    /// performance images. An omitted field is deliberately different from a
    /// diagnostic snapshot whose counters all happened to be zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) access_point: Option<oer_hil_protocol::wifi::WifiAccessPointEvidence>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(super) enum TrafficReport {
    None,
    Icmp {
        transmitted: u16,
        received: u16,
        lost: u16,
        p50_micros: u64,
        p95_micros: u64,
        p99_micros: u64,
    },
    Udp(SessionReport),
    UdpMultiClient(Box<MultiClientSessionReport>),
    Tcp(SessionReport),
}

#[derive(Serialize)]
pub(super) struct SessionReport {
    pub(super) direction: Direction,
    pub(super) rx_bytes: u64,
    pub(super) tx_bytes: u64,
    pub(super) rx_units: u64,
    pub(super) tx_units: u64,
    pub(super) elapsed_micros: u64,
}

#[derive(Serialize)]
pub(super) struct MultiClientSessionReport {
    pub(super) direction: Direction,
    pub(super) aggregate: SessionReport,
    pub(super) flows: [MultiClientFlowReport; SESSION_FLOW_CAPACITY],
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct MultiClientFlowReport {
    pub(super) flow_id: u8,
    pub(super) peer: Ipv4Endpoint,
    pub(super) rx_bytes: u64,
    pub(super) tx_bytes: u64,
    pub(super) rx_units: u64,
    pub(super) tx_units: u64,
    pub(super) elapsed_micros: u64,
    pub(super) rx_bps: u64,
    pub(super) tx_bps: u64,
    pub(super) host_tx_started_at_zero: Option<bool>,
    pub(super) host_tx_missing: Option<u64>,
    pub(super) host_tx_reordered: Option<u64>,
    pub(super) host_tx_duplicates: Option<u64>,
    pub(super) host_tx_maximum_interarrival_us: Option<u64>,
    pub(super) host_tx_sequence_after_maximum_interarrival: Option<u32>,
}
