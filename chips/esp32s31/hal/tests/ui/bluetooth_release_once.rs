#![no_std]
// ui-requires: bluetooth-engine
use oer_esp32s31_hal::bluetooth::engine::{DtmPool, SchedulerRoleInstance};

pub fn release<'pool, 'epoch>(
    pool: &mut DtmPool<'pool, 'epoch, 1>,
    lease: SchedulerRoleInstance<'pool, 'epoch>,
) {
    let _ = pool.release(lease);
    #[cfg(ui_reject)]
    let _ = pool.release(lease); // ui-error: E0382
}
