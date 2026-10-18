//! Per-interface diagnostic storage and post-workload logging.
use super::progress::{Counters, Event, Snapshot};
use crate::console::runtime_log_reliably;
use oer_hil_esp32s31_telemetry::wait;
use oer_hil_protocol::wifi::WifiNetworkInterface;

static STATION: Counters = Counters::new();
static ACCESS_POINT: Counters = Counters::new();

pub(in crate::product_hil) fn counters(role: WifiNetworkInterface) -> &'static Counters {
    match role {
        WifiNetworkInterface::Station => &STATION,
        WifiNetworkInterface::AccessPoint => &ACCESS_POINT,
    }
}

pub(in crate::product_hil) async fn log(
    role: WifiNetworkInterface,
    sample: Snapshot,
    send: wait::Snapshot,
    pacing: wait::Snapshot,
) {
    runtime_log_reliably(format_args!(
        "hil-net: role={role:?} polls={} no_transfer={}",
        sample.get(Event::NetworkPoll),
        sample.get(Event::PollWithoutTransfer),
    ))
    .await;
    runtime_log_reliably(format_args!(
        "hil-net: tx_ready={} tx_unavailable={} tx_accepted={} tx_rejected={}",
        sample.get(Event::TxReady),
        sample.get(Event::TxUnavailable),
        sample.get(Event::TxAccepted),
        sample.get(Event::TxRejected),
    ))
    .await;
    runtime_log_reliably(format_args!(
        "hil-net: rx_empty={} rx_delivered={}",
        sample.get(Event::RxEmpty),
        sample.get(Event::RxDelivered),
    ))
    .await;
    for (name, wait) in [("udp_send", send), ("pacing", pacing)] {
        runtime_log_reliably(format_args!(
            "hil-net: op={name} polls={} pending={} completed={} cancelled={}",
            wait.polls, wait.pending, wait.completed, wait.cancelled,
        ))
        .await;
        runtime_log_reliably(format_args!(
            "hil-net: op={name} poll_us={} suspended_us={}",
            wait.poll_micros, wait.suspended_micros,
        ))
        .await;
    }
}
