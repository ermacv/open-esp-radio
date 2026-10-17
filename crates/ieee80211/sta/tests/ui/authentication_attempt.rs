#![no_std]
use oer_ieee80211_mac::station::StaSequenceCounter;
use oer_ieee80211_sta::join::authentication::{StaAuthenticationEvent, StaAuthenticationRuntime};

pub fn retry(ready: StaAuthenticationRuntime, sequence: &mut StaSequenceCounter) {
    let (active, _) = ready.begin_attempt(sequence);
    #[cfg(ui_reject)]
    let _ = active.begin_attempt(sequence); // ui-error: E0599
    #[cfg(not(ui_reject))]
    if let StaAuthenticationEvent::Retry { runtime, .. } = active.response_timed_out() {
        let _ = runtime.begin_attempt(sequence);
    }
}
