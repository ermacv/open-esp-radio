#![no_std]
// ui-requires: bluetooth-engine
use oer_esp32s31_hal::bluetooth::engine::*;

pub fn dtm(engine: &mut Engine<ArenaMemory<'_, '_, 0, 0, 0, 0, 0, 0, 1>>) {
    let _ = engine.dtm();
    #[cfg(ui_reject)]
    let _ = engine.scanning(); // ui-error: E0599
    #[cfg(ui_reject)]
    let _ = engine.advertising(); // ui-error: E0599
    #[cfg(ui_reject)]
    let _ = engine.connectable_advertising(); // ui-error: E0599
    #[cfg(ui_reject)]
    let _ = engine.connections(); // ui-error: E0599
}

pub fn full(engine: &mut Engine<ArenaMemory<'_, '_, 1, 1, 1, 1, 2, 2, 4>>) {
    let _ = engine.scanning();
    let _ = engine.advertising();
    let _ = engine.connectable_advertising();
    let _ = engine.connections();
}
