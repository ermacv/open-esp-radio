#![forbid(unsafe_code)]

mod rx;
mod tx;

pub(in crate::product_hil) use rx::{
    UdpRxBenchmarkConfig, UdpRxSessionSource, UdpRxTelemetry, run_open_radio_udp_rx_benchmark,
};
pub(in crate::product_hil) use tx::{
    UdpTxBenchmarkConfig, UdpTxSessionSource, configure_multi_flow_burst_datagrams,
    multi_flow_burst_datagrams, run_open_radio_udp_tx_benchmark,
};
