//! PHY transaction cancellation: quarantine, retained reset and next-boot output.
use oer_esp32s31_hal::domain::transaction::FaultRecord;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::domain::transaction::TransactionSeat;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::domain::transaction::{Fault, FaultReason, PhyIdentity};
crate::retained_static! { static PHY_FAULT: FaultRecord; }
#[cfg(target_arch = "riscv32")]
static SEAT: TransactionSeat = TransactionSeat::new();

#[cfg(target_arch = "riscv32")]
pub fn identity() -> PhyIdentity {
    match SEAT.claim() {
        Some(identity) => identity,
        None => reset(Fault {
            reason: FaultReason::IdentityClaimed,
            owner: core::ptr::from_ref(&SEAT).addr(),
            registration: None,
        }),
    }
}
#[cfg(target_arch = "riscv32")]
pub fn reset(fault: Fault) -> ! {
    crate::fatal::reset_recording(|_stop| {
        if let Some(mut record) = PHY_FAULT.try_lock() {
            record.record(fault);
        }
    })
}
pub fn report_previous() {
    if let Some(mut record) = PHY_FAULT.try_lock() {
        let _reported = record.report(oer_esp32s31_hal::usb_serial_jtag::early_text);
    }
}

#[cfg(target_arch = "riscv32")]
oer_memory::link_fn! { define fn __oer_phy_transaction_fault(fault: Fault) -> ! = reset; }
