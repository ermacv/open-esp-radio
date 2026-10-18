//! HIL-owned stack composition; all implementations share the traffic workloads.
#![forbid(unsafe_code)]
pub(super) struct Settings {
    pub ipv4: Option<oer_hil_protocol::wifi::NetworkIpv4Configuration>,
    pub seed: u64,
    pub rx_checksum: oer_hil_protocol::wifi::WifiRxChecksumPolicy,
    pub tx_udp_checksum: oer_hil_protocol::wifi::WifiTxUdpChecksumPolicy,
}

mod embassy;
pub(super) use embassy::*;
#[cfg(feature = "task-poll-telemetry")]
pub(super) mod observation;
#[cfg(feature = "task-poll-telemetry")]
pub(super) use oer_hil_agent::network::progress;
pub(crate) use oer_hil_agent::network::sockets;
