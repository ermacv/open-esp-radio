#![no_std]
use oer_ieee80211_sta::ftm::FtmRequestTransmission;
use oer_time::Instant;
pub fn complete<const N: usize>(transmission: FtmRequestTransmission<N>, now: Instant) {
    let _first = transmission.complete(true, now);
    #[cfg(ui_reject)]
    let _again = transmission.complete(true, now); // ui-error: E0382
}
