#![no_std]
// ui-requires: bluetooth-engine
use oer_esp32s31_hal::bluetooth::engine::{DtmPool, SchedulerRoleInstance};

pub fn build<'pool, 'epoch>(
    pool: &mut DtmPool<'pool, 'epoch, 1>, lease: SchedulerRoleInstance<'pool, 'epoch>,
) {
    let value = pool.words(&lease);
    #[cfg(ui_reject)]
    let _ = pool.cpu(&lease); // ui-error: E0624
    let _ = pool.release(lease);
    // A detached value can outlive a lease without retaining a layout loan.
    let _ = value;
}
