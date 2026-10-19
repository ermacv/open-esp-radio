#![no_std]
use oer_ieee80211_lower_mac::Ieee80211Capabilities;
use oer_ieee80211_sta::machine::{RetiredStation, StaTokens, StaRequest, StaCommand};
use oer_time::Instant;

pub fn terminal<'id, T: StaTokens, K: Ieee80211Capabilities>(mut ended: RetiredStation<'id, T, K>) {
    #[cfg(ui_reject)]
    drop(ended.command(Instant::EPOCH, StaRequest::new(1, StaCommand::Connect))); // ui-error: E0599
    #[cfg(not(ui_reject))]
    drop(ended.poll_output());
}
