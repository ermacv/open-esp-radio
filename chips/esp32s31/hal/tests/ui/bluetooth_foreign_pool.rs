#![no_std]
// ui-requires: bluetooth-engine
use oer_esp32s31_hal::bluetooth::engine::{DtmPool, SchedulerRoleInstance};

// Same storage, capacity and epoch: only the pool brand differs.
pub fn release<'a, 'b, 'epoch>(
    a: &mut DtmPool<'a, 'epoch, 1>,
    b: &mut DtmPool<'b, 'epoch, 1>,
    lease: SchedulerRoleInstance<'a, 'epoch>,
) {
    #[cfg(ui_reject)]
    let _ = b.release(lease); // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    let _ = a.release(lease);
}
