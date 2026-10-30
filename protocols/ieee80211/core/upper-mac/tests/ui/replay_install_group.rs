#![no_std]
use oer_ieee80211_core_upper_mac::ccmp::{CcmpRxReplayEpoch};
use oer_ieee80211_wire::ccmp::GroupKeyId;
pub fn rewind(epoch: &mut CcmpRxReplayEpoch, id: GroupKeyId) {
    #[cfg(ui_reject)]
    epoch.install_group(id, [0; 8]); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _installed = epoch.has_group(id);
}
