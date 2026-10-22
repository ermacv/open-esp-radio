#![no_std]
// ui-requires: bluetooth-engine
use oer_esp32s31_hal::{bluetooth::{ControllerHal, engine::*}, dma_static::Placed};

pub fn install<'registers, 'pool, 'epoch>(
    raw: ControllerHal<'registers>, ready: EngineRegisters<'registers>,
    arena: Placed<BluetoothArena<1, 1, 1, 2, 2>>,
    pool: PoolScope<'pool>, epoch: EpochScope<'epoch>, config: SchedulerAllocationConfig,
) {
    #[cfg(ui_reject)]
    let _ = Engine::<1, 1, 1, 2, 2, 4>::new::<HostDma<0x2f00_1000>>(raw, arena, pool, epoch, config); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _ = Engine::<1, 1, 1, 2, 2, 4>::new::<HostDma<0x2f00_1000>>(ready, arena, pool, epoch, config);
}
