#![forbid(unsafe_code)]

mod bidirectional;
#[cfg(any(
    feature = "core0-rx-cycle-telemetry",
    feature = "core0-rx-coarse-telemetry"
))]
mod cache_performance;
mod cpu_load;
mod evidence;
mod reporting;
mod runtime;
mod tcp;
mod udp;

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use oer_tool_data_hil_protocol::network::SessionLinkRequirements;
use oer_tool_data_hil_protocol::network::Transport;
use oer_tool_data_hil_protocol::wifi::WifiNetworkInterface;

use crate::console::{ActiveSession, receive_session_start};

pub(super) type SessionChannel = Channel<CriticalSectionRawMutex, ActiveSession, 1>;

async fn run_session_dispatcher(
    station_udp: &'static SessionChannel,
    station_tcp: &'static SessionChannel,
    access_point_udp: &'static SessionChannel,
    access_point_tcp: &'static SessionChannel,
) -> ! {
    loop {
        let session = receive_session_start().await;
        let (udp, tcp) = match session.config.network_interface {
            WifiNetworkInterface::Station => (station_udp, station_tcp),
            WifiNetworkInterface::AccessPoint => (access_point_udp, access_point_tcp),
        };
        match session.config.transport {
            Transport::Udp => udp.send(session).await,
            Transport::Tcp => tcp.send(session).await,
        }
    }
}

/// Wait until the production control/TX owners have proved every link
/// property requested by a measured session. There is deliberately no
/// fallback timeout into a slower mode: the host owns the qualification
/// timeout and must classify an unavailable AddBA precondition separately
/// from transport performance.
async fn wait_session_link_requirements(
    requirements: SessionLinkRequirements,
    network_interface: WifiNetworkInterface,
) {
    let Some(tid) = requirements.tx_block_ack_tid else {
        return;
    };
    assert_eq!(
        network_interface,
        WifiNetworkInterface::Station,
        "only the station role currently exports a functional TX BlockAck status",
    );
    let bit = 1_u32
        .checked_shl(u32::from(tid))
        .expect("validated BlockAck TID fits the functional status bitmap");
    let mut receiver = crate::product_hil::STATION_TX_BLOCK_ACK_OPERATIONAL_TIDS
        .receiver()
        .expect("station UDP and TCP link waiters");
    receiver.get_and(|operational| operational & bit != 0).await;
}

pub(super) use bidirectional::{
    BidirectionalResultChannel, BidirectionalSessionChannel, OpenRadioBidirectionalDirection,
    OpenRadioBidirectionalResult, complete_open_radio_bidirectional_direction,
    run_open_radio_bidirectional_session_coordinator,
};
pub(super) use cpu_load::CpuLoadWindow;
pub(super) use evidence::{UdpSequenceEvidence, iperf2_udp_sequence};
#[cfg(any(
    feature = "core0-rx-coarse-telemetry",
    feature = "core0-rx-cycle-telemetry"
))]
pub(super) use reporting::observe_open_radio_core0_task_polls;
pub(super) use reporting::{
    aggregate_tx_evidence, log_open_radio_ampdu_snapshot, log_open_radio_rx_pipeline_interval,
    log_open_radio_task_poll_interval, observe_open_radio_task_polls,
};
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub(super) use reporting::{log_open_radio_core0_rx_coarse, log_open_radio_tx_promotion};
#[cfg(feature = "core0-rx-cycle-telemetry")]
pub(super) use reporting::{
    log_open_radio_core0_rx_cycles, log_open_radio_core0_rx_service_histogram,
};
pub(in crate::product_hil) use runtime::{start_connected_traffic, start_traffic_dispatcher};
pub(super) use tcp::{TcpBenchmarkConfig, run_open_radio_tcp_benchmark};
pub(super) use udp::{
    UdpRxBenchmarkConfig, UdpRxSessionSource, UdpRxTelemetry, UdpTxBenchmarkConfig,
    UdpTxSessionSource, configure_multi_flow_burst_datagrams, multi_flow_burst_datagrams,
    run_open_radio_udp_rx_benchmark, run_open_radio_udp_tx_benchmark,
};

/// The session's traffic ended: its evidence must now be published. A task
/// that stops before publishing becomes a named hang instead of silence.
pub(in crate::product_hil) fn session_evidence_due() {
    #[cfg(not(feature = "memory-benchmark"))]
    crate::hang_watchdog::arm(oer_tool_data_hil_protocol::base::TaskSlot::SessionEvidence);
}

/// The session's evidence reached the protocol owner.
pub(in crate::product_hil) fn session_evidence_published() {
    #[cfg(not(feature = "memory-benchmark"))]
    crate::hang_watchdog::took_work(oer_tool_data_hil_protocol::base::TaskSlot::SessionEvidence, false);
}
