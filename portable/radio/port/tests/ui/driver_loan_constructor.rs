#![no_std]
use oer_radio_port::DriverLoan;
pub fn driver<D>(driver: &mut D) {
    #[cfg(ui_reject)]
    let _loan = DriverLoan { driver }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let _view = &*driver;
}
