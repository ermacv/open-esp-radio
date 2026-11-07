#![no_std]
use oer_chips_esp32s31_radio_ieee80211_backend::{lower_mac::*, ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile}};
use oer_portable_time::Timer;

pub fn borrow<'core, 'slot, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, C: oer_chips_esp32s31_radio_ieee80211_backend::lower_mac::RadioCoexPriorities>(
    owner: &mut CoreOwner<LowerMacCore<'core, 'slot, P, E, T, C, 512>>,
) {
    #[cfg(ui_reject)]
    let _view = EnabledView::<_, OpenGate>::new(owner); // ui-error: E0624
    #[cfg(not(ui_reject))]
    if let CoreView::Open(view) = owner.view() {
        let _deadline = view.next_deadline();
    }
}
