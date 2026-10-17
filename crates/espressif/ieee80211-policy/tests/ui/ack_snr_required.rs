#![no_std]
use oer_espressif_ieee80211_policy::rate_selection::EspressifRateSelection;
use oer_ieee80211_lower_mac::{AckSnr, Ieee80211Capabilities, SignalMetadata};
use oer_ieee80211_upper_mac::rate_selection::RateSelection;

fn for_port<M: SignalMetadata, A>(_: impl RateSelection<M, A>) {}

/// ACK evidence that carries the response's SNR.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snr(i8);

impl AckSnr for Snr {
    fn snr_db(&self) -> i8 {
        self.0
    }
}

/// ACK evidence of a backend that measures nothing of the response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unmeasured;

pub fn select<K: Ieee80211Capabilities, M: SignalMetadata>() {
    #[cfg(ui_reject)]
    for_port::<M, Unmeasured>(EspressifRateSelection::<K>::new()); // ui-error: E0277
    #[cfg(not(ui_reject))]
    for_port::<M, Snr>(EspressifRateSelection::<K>::new());
}
