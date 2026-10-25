#![no_std]
use oer_esp32s31_ieee80211_mac::irq::epoch::{InterruptEpoch, Moderated, Unmoderated};
pub fn unmask(moderated: &mut InterruptEpoch<Moderated>, unmoderated: &mut InterruptEpoch<Unmoderated>) {
    #[cfg(not(ui_reject))]
    let _restored = moderated.unmask_rx_after_drain();
    #[cfg(ui_reject)]
    let _restored = unmoderated.unmask_rx_after_drain(); // ui-error: E0599
    let _ = (moderated, unmoderated);
}
