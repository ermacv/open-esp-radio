#![no_std]
use {oer_ieee80211_wire::ccmp::{CcmpHeader, GroupKeyId}, oer_ieee80211_upper_mac::ccmp::{CcmpRxProtection, CcmpRxReplayEpoch, CcmpRxReplayState}};

pub fn replace(epoch: &mut CcmpRxReplayEpoch, header: CcmpHeader, id: GroupKeyId) {
    let candidate = epoch
        .prepare(CcmpRxProtection::Group, None, header)
        .unwrap();
    #[cfg(ui_reject)]
    {
        epoch.remove_group(id); // ui-error: E0499
        let _previous = epoch.exchange_group(id, CcmpRxReplayState::default()); // ui-error: E0499
        candidate.commit();
    }
    #[cfg(not(ui_reject))]
    {
        candidate.commit();
        epoch.remove_group(id);
        let _previous = epoch.exchange_group(id, CcmpRxReplayState::default());
    }
}
