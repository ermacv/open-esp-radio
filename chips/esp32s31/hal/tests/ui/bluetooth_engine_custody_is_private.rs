#![no_std]
// ui-requires: bluetooth-engine
use oer_esp32s31_hal::bluetooth::engine::*;

pub fn access(engine: &mut Engine<ArenaMemory<'_, '_, 0, 1, 2, 4, 4, 8>>) {
    let _ = engine.dtm();
    #[cfg(ui_reject)]
    let _ = &mut engine.registers; // ui-error: E0616
    #[cfg(ui_reject)]
    let _ = &mut engine.memory; // ui-error: E0616
    #[cfg(ui_reject)]
    let _ = engine.controller(); // ui-error: E0624
}

pub fn quarantine<M: ControllerMemory>(failure: EngineShutdownFailure<M>) {
    let _ = failure.error();
    #[cfg(ui_reject)]
    let _ = failure.into_parts(); // ui-error: E0599
}
