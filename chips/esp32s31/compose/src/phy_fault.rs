//! PHY transaction cancellation: quarantine, retained reset and next-boot output.
use oer_diagnostics::fault::Record;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::domain::transaction::TransactionSeat;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::domain::transaction::encode_fault;
use oer_esp32s31_hal::domain::transaction::report_fault;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::domain::transaction::{Fault, FaultReason, PhyIdentity};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::platform::Port;
use oer_espressif_staged_runtime::retained_fault;
oer_espressif_staged_runtime::retained_static! { static PHY_FAULT: Record; }
#[cfg(target_arch = "riscv32")]
static SEAT: TransactionSeat = TransactionSeat::new();

#[cfg(target_arch = "riscv32")]
pub fn identity() -> PhyIdentity {
    match SEAT.claim() {
        Some(identity) => identity,
        None => reset(Fault {
            reason: FaultReason::IdentityClaimed,
            registration: None,
        }),
    }
}
#[cfg(target_arch = "riscv32")]
pub fn reset(fault: Fault) -> ! {
    oer_espressif_staged_runtime::fatal::reset_recording::<Port>(|claim, _stop| {
        if let Ok(record) = encode_fault(fault) {
            let _committed = claim.commit(&PHY_FAULT, record);
        }
    })
}
pub fn report_previous() {
    if let Some(mut record) = retained_fault::peek(&PHY_FAULT) {
        let _reported = report_fault(
            &mut record,
            oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text,
        );
    }
}

/// A copied observation, not a hardware owner or permission to continue.
/// An image may consume it through the platform's retained terminal path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Failure(oer_esp32s31_hal::domain::transaction::Fault);
impl Failure {
    pub fn capture(fault: oer_esp32s31_hal::domain::transaction::Fault) -> Self {
        Self(fault)
    }
    #[cfg(target_arch = "riscv32")]
    pub fn reset(self) -> ! {
        reset(self.0)
    }
}
