#![no_std]
use oer_radio_port::DriverLoan;
pub fn driver<D>(mut loan: DriverLoan<'_, D>, replacement: D) {
    #[cfg(ui_reject)]
    let _owner = core::mem::replace(&mut *loan, replacement); // ui-error: E0596
    #[cfg(not(ui_reject))]
    {
        let _view = &*loan;
        drop(replacement);
    }
}
