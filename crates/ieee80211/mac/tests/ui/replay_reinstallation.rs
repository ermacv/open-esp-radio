#![no_std]
use oer_ieee80211_mac::ccmp::{CcmpHeader, CcmpRxProtection, CcmpRxReplayEpoch, GroupKeyId};

pub fn replace(epoch: &mut CcmpRxReplayEpoch, header: CcmpHeader, id: GroupKeyId) {
    let candidate = epoch
        .prepare(CcmpRxProtection::Group, None, header)
        .unwrap();
    #[cfg(ui_reject)]
    {
        epoch.remove_group(id); // ui-error: E0499
        epoch.install_group(id, [0; 8]).unwrap(); // ui-error: E0499
        candidate.commit();
    }
    #[cfg(not(ui_reject))]
    {
        candidate.commit();
        epoch.remove_group(id);
        epoch.install_group(id, [0; 8]).unwrap();
    }
}
