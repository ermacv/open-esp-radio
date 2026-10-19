#![no_std]
use oer_ieee80211_lower_mac::{BlockAckSnr, SnrSample};
use oer_ieee80211_upper_mac::rate_control::{AmpduUnavailable, BlockAckTally, RateControl};

/// BlockAck evidence that splits into an SNR sample or none.
pub struct Measured(i8);

impl BlockAckSnr for Measured {
    fn snr_sample(&self) -> SnrSample {
        SnrSample::Measured(self.0)
    }
}

/// BlockAck evidence of a backend that does not say whether it measured.
pub struct Opaque;

pub fn observe<C: RateControl>(
    control: &mut C,
    now: oer_time::Instant,
    tally: BlockAckTally,
) -> Result<(), AmpduUnavailable> {
    #[cfg(ui_reject)]
    return control.observe_ampdu(now, tally, &Opaque); // ui-error: E0277
    #[cfg(not(ui_reject))]
    control.observe_ampdu(now, tally, &Measured(20))
}
