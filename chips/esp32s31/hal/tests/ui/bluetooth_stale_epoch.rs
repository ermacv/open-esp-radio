#![no_std]
// ui-requires: bluetooth-engine
use oer_esp32s31_hal::{
    bluetooth::{BluetoothControllerReset, engine::{with_epoch, with_pool, DtmStorage, HostDma, SchedulerAllocationNumbers, SchedulerRolePoolStorage}},
    dma_static::Placed,
};

pub fn reset_pool(
    storage: Placed<SchedulerRolePoolStorage<DtmStorage, 1>>,
    numbers: SchedulerAllocationNumbers,
    reset: &BluetoothControllerReset,
) {
    with_pool(|scope| with_epoch(|epoch| {
        let mut pool = scope.place::<HostDma<0x2f00_1000>, _, 1>(epoch, storage, numbers).unwrap();
        let old = pool.acquire().unwrap();
        #[cfg(not(ui_reject))]
        let _ = pool.release(old);
        with_epoch(|epoch| {
            let mut pool = pool.reset_after_controller_reset(reset, epoch).ok().unwrap();
            #[cfg(ui_reject)]
            let _ = pool.release(old); // ui-error: E0521
            let fresh = pool.acquire().unwrap();
            let _ = pool.release(fresh);
        });
    }));
}
