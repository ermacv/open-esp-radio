//! Explicit HIL tariff shared by the RR and deficit experiment arms.
use core::cell::RefCell;
use core::num::NonZeroU32;
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use oer_esp32s31_ieee80211_mac::tx::{LegacyRate, TxPhyRate};
use oer_esp32s31_ieee80211_runtime::roles::access_point::network_tx::{
    AccessPointAirtimeConfiguration, AccessPointAirtimePeer, AccessPointAirtimeSelection,
    AirtimeObservation,
};
use oer_hil_esp32s31_telemetry::airtime::AirtimeHistory;
use oer_hil_protocol::wifi::WifiApScheduler;
use oer_ieee80211_softmac::{MacTxWork, tx_cost::PpduTiming};

static HISTORY: Mutex<CriticalSectionRawMutex, RefCell<Option<AirtimeHistory>>> =
    Mutex::new(RefCell::new(None));

fn observe(event: AirtimeObservation<AccessPointAirtimePeer>) {
    HISTORY.lock(|history| {
        if let Some(history) = history.borrow_mut().as_mut() {
            history.observe(event);
        }
    });
}

pub(super) fn reset() {
    HISTORY.lock(|history| {
        let mut history = history.borrow_mut();
        if history.is_some() {
            *history = Some(AirtimeHistory::new());
        }
    });
}

/// Called after the radio stop edge. Copy each bounded record under the lock,
/// then await the existing reliable event transport with no lock held.
pub(super) async fn report(request_id: u32) {
    for index in 0..8 {
        let peer = HISTORY.lock(|history| history.borrow().as_ref().and_then(|h| h.peers[index]));
        if let Some(peer) = peer {
            crate::console::publish_event_reliably(
                0,
                request_id,
                oer_hil_protocol::wifi::AirtimePeer(peer),
            )
            .await;
        }
    }
    let report = HISTORY.lock(|history| history.borrow().as_ref().map(|h| h.report));
    if let Some(report) = report {
        crate::console::publish_event_reliably(
            0,
            request_id,
            oer_hil_protocol::wifi::AirtimeReport(report),
        )
        .await;
    }
}

fn response(_: AccessPointAirtimePeer) -> Option<PpduTiming> {
    TxPhyRate::Legacy(LegacyRate::Ofdm24M).ppdu_timing()
}

fn cost(peer: AccessPointAirtimePeer, work: &MacTxWork) -> Option<NonZeroU32> {
    // A uniform compressed-BA-sized envelope also charges ordinary ACK
    // exchanges. It is an explicit comparison tariff, not recovered ACK PHY
    // or an estimate of contention, protection or hidden MAC retry time.
    let overhead = match peer {
        AccessPointAirtimePeer::Group => 0,
        AccessPointAirtimePeer::Unicast(_) => 10 + response(peer)?.duration_micros(32),
    };
    work.estimated_exchange_micros(overhead)
}

pub(super) fn configuration(policy: WifiApScheduler) -> Option<AccessPointAirtimeConfiguration> {
    let selection = match policy {
        WifiApScheduler::Disabled => return None,
        WifiApScheduler::RrHtResponse24 => AccessPointAirtimeSelection::RoundRobin,
        WifiApScheduler::DeficitHtResponse24 => AccessPointAirtimeSelection::Deficit,
    };
    HISTORY.lock(|history| history.replace(Some(AirtimeHistory::new())));
    Some(AccessPointAirtimeConfiguration {
        selection,
        quantum_micros: NonZeroU32::new(3000).unwrap(),
        minimum_exchange_micros: NonZeroU32::new(100).unwrap(),
        cost,
        block_ack_timing: response,
        observer: Some(observe),
    })
}
