#![no_std]
use oer_ieee80211_sta::ftm::FtmRequestTransmission;
pub fn body<const N: usize>(mut transmission: FtmRequestTransmission<N>) {
    #[cfg(ui_reject)]
    { transmission.body[0] = 0; } // ui-error: E0616
    #[cfg(not(ui_reject))]
    let _category = transmission.body()[0];
    let _terminal = transmission.cancel();
}
