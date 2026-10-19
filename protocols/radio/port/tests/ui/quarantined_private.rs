#![no_std]
use oer_radio_port::Quarantined;
pub fn retained<S>(state: S) {
    let receipt = Quarantined::new(state);
    #[cfg(ui_reject)]
    let _owners = receipt._retained; // ui-error: E0616
    #[cfg(not(ui_reject))]
    drop(receipt);
}
