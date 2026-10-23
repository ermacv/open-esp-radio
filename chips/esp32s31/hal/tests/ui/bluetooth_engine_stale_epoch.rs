#![no_std]
// ui-requires: bluetooth-engine
use oer_esp32s31_hal::bluetooth::engine::*;

pub fn release<'pool, 'old, 'next>(
    old: &mut Engine<ArenaMemory<'pool, 'old, 0, 1, 2, 1, 4, 4, 8>>,
    next: &mut Engine<ArenaMemory<'pool, 'next, 0, 1, 2, 1, 4, 4, 8>>,
    lease: SchedulerRoleInstance<'pool, 'old>,
) {
    #[cfg(not(ui_reject))]
    let _ = old.dtm().unwrap().release(lease);
    #[cfg(ui_reject)]
    let _ = next.dtm().unwrap().release(lease); // ui-error: lifetime may not live long enough
}
