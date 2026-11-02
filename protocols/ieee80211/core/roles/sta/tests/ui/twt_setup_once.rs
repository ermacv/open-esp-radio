#![no_std]
use oer_protocols_ieee80211_core_roles_sta::twt::{IndividualTwtIdleFlow, IndividualTwtProposal};
use oer_portable_time::Instant;

pub fn setup(idle: IndividualTwtIdleFlow<'_>, proposal: IndividualTwtProposal, now: Instant) {
    let _ = idle.queue_setup(proposal, now);
    #[cfg(ui_reject)]
    let _ = idle.queue_setup(proposal, now); // ui-error: E0382
}
