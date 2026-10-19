#![no_std]
use oer_ieee80211_mac::station::StaSequenceCounter;
use oer_ieee80211_sta::join::association::StaAssociationRuntime;
use oer_time::Instant;

pub fn poll(runtime: StaAssociationRuntime, now: Instant, sequence: &mut StaSequenceCounter) {
    let _outcome = runtime.poll(now, sequence);
    #[cfg(ui_reject)]
    let _again = runtime.poll(now, sequence); // ui-error: E0382
}
