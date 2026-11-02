#![no_std]
use oer_protocols_ieee80211_core_upper_mac::sequence::SequenceCounter;
use oer_protocols_ieee80211_core_roles_sta::join::association::StaAssociationRuntime;
use oer_portable_time::Instant;

pub fn poll(runtime: StaAssociationRuntime, now: Instant, sequence: &mut SequenceCounter) {
    let _outcome = runtime.poll(now, sequence);
    #[cfg(ui_reject)]
    let _again = runtime.poll(now, sequence); // ui-error: E0382
}
