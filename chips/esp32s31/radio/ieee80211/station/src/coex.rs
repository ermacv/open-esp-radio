//! Which shared-table event a Wi-Fi operation requests.

use oer_chips_family_coex::{CoexEventId, wifi_events};
use oer_protocols_ieee80211_contracts_port::{
    ConnectionFrame, Operation as WifiOperation, TxOperation,
};

/// The shared-table event an IEEE 802.11 operation requests, or `None` for
/// a data frame, which takes the MAC's static per-access-category priority
/// and requests no event.
///
/// - `BeaconWindow`: event 0, `coex_pti_get(0)` that Wi-Fi's power
///   management reads on every beacon.
/// - `Connection(ProbeRequest)`: event 45 and `Connection(Authentication |
///   Association | Eapol)`: event 46, when the reconnect policy is on.
///
/// SOURCE(esp32s31): complete pinned `libpp[pp.o]::pp_coex_tx_request` and
/// `libpp[pm_coex.o]::pm_coex_reconnect_policy`; the data row is
/// `libpp[hal_mac.o]` vendor data encapsulation
/// (`LegacyTxQueue::vendor_data_packet_priority` of the S31 MAC).
pub const fn wifi_event(operation: WifiOperation) -> Option<CoexEventId> {
    match operation {
        WifiOperation::BeaconWindow => Some(wifi_events::BEACON),
        WifiOperation::Transmit(TxOperation::Data) => None,
        WifiOperation::Transmit(TxOperation::Connection(ConnectionFrame::ProbeRequest)) => {
            Some(wifi_events::PROBE_REQUEST)
        }
        WifiOperation::Transmit(TxOperation::Connection(
            ConnectionFrame::Authentication | ConnectionFrame::Association | ConnectionFrame::Eapol,
        )) => Some(wifi_events::CONNECTION),
    }
}

#[cfg(test)]
mod tests;
