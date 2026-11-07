#![no_std]
// ui-requires: bluetooth-engine
use oer_chips_esp32s31_hal::radio::bluetooth::engine::SchedulerItemSpace;
#[cfg(ui_reject)]
use oer_chips_esp32s31_hal::radio::bluetooth::engine::SchedulerItemSources; // ui-error: E0603

pub fn empty() -> SchedulerItemSpace {
    SchedulerItemSpace::new()
}
