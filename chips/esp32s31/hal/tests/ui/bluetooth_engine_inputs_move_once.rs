#![no_std]
// ui-requires: bluetooth-engine
use oer_esp32s31_hal::{
    bluetooth::{ClockedOwner, engine::*},
    dma_static::Placed,
};

pub fn install<'pool, 'epoch>(
    clocked: ClockedOwner,
    arena: Placed<BluetoothArena<0, 1, 2, 4, 4>>,
    pool: PoolScope<'pool>,
    epoch: EpochScope<'epoch>,
    config: SchedulerAllocationConfig,
) {
    let _result = Engine::<ArenaMemory<'pool, 'epoch, 0, 1, 2, 4, 4, 8>>::new::<HostDma<0x2f00_1000>>(
        clocked, arena, pool, epoch, config,
    );
    #[cfg(ui_reject)]
    drop(clocked); // ui-error: E0382
    #[cfg(ui_reject)]
    drop(arena); // ui-error: E0382
}
