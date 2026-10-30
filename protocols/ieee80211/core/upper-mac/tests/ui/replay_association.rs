#![no_std]
use {oer_ieee80211_wire::ccmp::CcmpHeader, oer_ieee80211_core_upper_mac::ccmp::{CcmpRxProtection, CcmpRxReplayEpoch}};

pub fn publish(old: &mut CcmpRxReplayEpoch, current: &mut CcmpRxReplayEpoch, header: CcmpHeader) {
    let candidate = old
        .prepare(CcmpRxProtection::Pairwise, None, header)
        .unwrap();
    #[cfg(ui_reject)]
    current.commit(candidate); // ui-error: E0599
    #[cfg(not(ui_reject))]
    candidate.commit();
}
