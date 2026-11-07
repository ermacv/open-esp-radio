#![no_std]
use oer_chips_esp32s31_hal::radio::domain::transaction::{FaultHandle, FaultReason, Operation};
pub fn observe(handle: FaultHandle) {
    handle.require_owner(Some(()));
    #[cfg(ui_reject)]
    handle.reset(FaultReason::Cancelled(Operation::Register)); // ui-error: E0599
}
