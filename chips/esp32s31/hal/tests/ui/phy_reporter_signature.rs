#![no_std]
use oer_esp32s31_hal::domain::transaction::Fault;
#[cfg(not(ui_reject))]
fn report(_: Fault) -> ! { loop {} }
#[cfg(ui_reject)]
fn report(_: Fault) {}
oer_esp32s31_hal::phy_fault_reporter!(report); // ui-error: E0308
