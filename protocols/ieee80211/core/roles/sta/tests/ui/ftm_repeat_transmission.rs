#![no_std]
use oer_protocols_ieee80211_core_roles_sta::ftm::FtmRequestTransmission;
use oer_portable_time::Instant; use oer_protocols_ieee80211_core_upper_mac::rate_control::MpduOutcome;
pub fn complete<const N: usize>(transmission: FtmRequestTransmission<N>, now: Instant) {
    let _first = transmission.complete(MpduOutcome::Acknowledged, now);
    #[cfg(ui_reject)]
    let _again = transmission.complete(MpduOutcome::Acknowledged, now); // ui-error: E0382
}
