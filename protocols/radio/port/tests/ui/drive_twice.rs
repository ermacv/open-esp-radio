#![no_std]
use oer_radio_port::{PortBackend, Retire};
pub fn drive<'p, 'id, B: PortBackend<Driver = ()>>(
    driver: oer_radio_port::DriverLoan<'p, B::Driver>,
    held: &'p B::Held<'id>,
    first: Retire<'p>,
    second: Retire<'p>,
) {
    let running = B::drive(driver, held, first);
    #[cfg(ui_reject)]
    let _ = B::drive(driver, held, second); // ui-error: E0382
    drop(running);
}
