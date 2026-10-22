#![no_std]
// ui-requires: bluetooth-engine
use oer_esp32s31_hal::bluetooth::engine::{ControllerMemory, Engine};
use oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::SchedulerInterruptHalf;

pub fn retire<M: ControllerMemory>(engine: &mut Engine<M>, half: SchedulerInterruptHalf) {
    let _ = engine.retire_scheduler_interrupts(half);
    #[cfg(ui_reject)]
    let _ = engine.retire_scheduler_interrupts(half); // ui-error: E0382
}
