#![no_std]
use oer_ieee80211_mac::ccmp::{CcmpHeader, CcmpRxProtection, CcmpRxReplayEpoch};

pub fn publish(old: &mut CcmpRxReplayEpoch, current: &mut CcmpRxReplayEpoch, header: CcmpHeader) {
    let candidate = old
        .prepare(CcmpRxProtection::Pairwise, None, header)
        .unwrap();
    #[cfg(ui_reject)]
    current.commit(candidate); // ui-error: E0599
    #[cfg(not(ui_reject))]
    candidate.commit();
}
