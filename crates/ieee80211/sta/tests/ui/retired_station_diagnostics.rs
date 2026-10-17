#![no_std]
use oer_ieee80211_lower_mac::Ieee80211Capabilities;
use oer_ieee80211_sta::machine::{RetiredStation, StaTokens};

pub fn product_bank<'id, T: StaTokens, K: Ieee80211Capabilities>(mut bank: RetiredStation<'id, T, K>) {
    #[cfg(ui_reject)]
    let _ = bank.frame_credits(); // ui-error: E0599
    #[cfg(ui_reject)]
    let _ = bank.key_owners(); // ui-error: E0599
    #[cfg(ui_reject)]
    let _ = bank.lost_tokens(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _ = bank.poll_output();
}
