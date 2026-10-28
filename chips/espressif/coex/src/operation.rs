//! Which priority each client's operation takes: the table of
//! (client, operation) rows.
//!
//! Each client has one exhaustive `match`, so a new operation cannot compile
//! without a row. A row carries its vendor `SOURCE`, or states
//! `product choice` and why. Whether an operation is raised at a given time
//! (for example a reconnecting station's connection frames) is state of the
//! coexistence owner, never of the row.

use crate::pti::{CoexEventId, ieee802154_events, wifi_events};
use oer_ieee80211_port::{ConnectionFrame, Operation as WifiOperation, TxOperation};
use oer_ieee802154_mac::RadioOperation;

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

/// The shared-table event an IEEE 802.15.4 operation reads.
///
/// The pinned archives' `coex_ieee802154_pti_get` reads the priority of
/// level `n` at event `40 + n`; `ieee802154_mac_init` and
/// `ieee802154_set_txrx_pti` give the idle scene level 4 (event 44),
/// immediate transmission and reception level 3 (event 43), timed
/// transmission and reception level 2 (event 42) and the ACK level 2.
///
/// SOURCE(esp32s31, esp32c5): esp-coex-lib
/// `fa152b3b6b2e3c5e3a57e55fcf4b005a4e28cef9` `coexist_api.o`, whose body is
/// equal in `esp32s31/libcoexist.a` sha256
/// `01c1b4f2b86cbc3024a2b51f3f104ea7937fc721a4ee57b53146fea6608836bb` and
/// `esp32c5/libcoexist.a` sha256
/// (esp-coex-lib `c758e7b56e0fa22177a0539796e1df59978dc322`)
/// `de883d649e4e2e9407a6ab0dc28d5d2aa4cd8d30d0071abfe5a9183348ceb627`;
/// ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
/// `esp_ieee802154_util.c` (`s_coex_config`).
///
/// `CslSlot`: product choice: the archive names no CSL row; a slot is a
/// timed reception, so it reads the timed-reception event.
pub const fn ieee802154_event(operation: RadioOperation) -> CoexEventId {
    match operation {
        RadioOperation::Idle => ieee802154_events::IDLE,
        RadioOperation::Tx | RadioOperation::Rx => ieee802154_events::IMMEDIATE,
        RadioOperation::TxAt | RadioOperation::RxAt | RadioOperation::Ack => {
            ieee802154_events::TIMED
        }
        RadioOperation::CslSlot => ieee802154_events::TIMED,
    }
}

#[cfg(test)]
mod tests;
