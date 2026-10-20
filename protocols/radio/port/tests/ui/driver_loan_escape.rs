#![no_std]
use oer_radio_port::DriverLoan;
#[cfg(ui_reject)]
pub fn escape<'p, D>(driver: DriverLoan<'p, D>) -> &'p D {
    &*driver // ui-error: E0515
}
#[cfg(not(ui_reject))]
pub fn borrow<'s, D>(driver: &'s DriverLoan<'_, D>) -> &'s D {
    driver
}
