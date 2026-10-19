#![no_std]
use oer_radio_port::DriverLoan;
pub fn driver<D>(loan: DriverLoan<'_, D>) {
    #[cfg(ui_reject)]
    let _owner = *loan; // ui-error: E0507
    #[cfg(not(ui_reject))]
    let _view = &*loan;
}
