#![no_std]
use oer_ieee80211_mac::twt::IndividualTwtFlowId;
use oer_ieee80211_sta::twt::IndividualTwtRequester;

pub fn select(owner: &mut IndividualTwtRequester, flow: IndividualTwtFlowId) {
    let first = owner.flow(flow);
    #[cfg(ui_reject)]
    let second = owner.flow(flow); // ui-error: E0499
    drop(first);
    #[cfg(ui_reject)]
    drop(second);
    let _ = owner.flow(flow);
}
