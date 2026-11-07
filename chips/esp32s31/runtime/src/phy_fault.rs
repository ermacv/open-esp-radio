//! PHY transaction cancellation: quarantine, retained reset and next-boot output.
#[cfg(target_arch = "riscv32")]
use oer_chips_esp32s31_hal::radio::domain::transaction::TransactionSeat;
#[cfg(target_arch = "riscv32")]
use oer_chips_esp32s31_hal::radio::domain::transaction::encode_fault;
use oer_chips_esp32s31_hal::radio::domain::transaction::report_fault;
#[cfg(target_arch = "riscv32")]
use oer_chips_esp32s31_hal::radio::domain::transaction::{Fault, FaultReason, PhyIdentity};
#[cfg(target_arch = "riscv32")]
use oer_chips_esp32s31_hal::system::platform::Port;
use oer_chips_family_staged_runtime::retained_fault;
use oer_portable_diagnostics::fault::Record;
oer_chips_family_staged_runtime::retained_static! { static PHY_FAULT: Record; }
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
    oer_chips_family_staged_runtime::fatal::reset_recording::<Port>(|claim, _stop| {
        if let Ok(record) = encode_fault(fault) {
            let _committed = claim.commit(&PHY_FAULT, record);
        }
    })
}
pub fn report_previous() {
    if let Some(mut record) = retained_fault::peek(&PHY_FAULT) {
        let _reported = report_fault(&mut record, |bytes: &[u8]| {
            oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::early_text(bytes).is_whole()
        });
    }
}

/// A copied observation, not a hardware owner or permission to continue.
/// An image may consume it through the platform's retained terminal path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Failure(oer_chips_esp32s31_hal::radio::domain::transaction::Fault);
impl Failure {
    pub fn capture(fault: oer_chips_esp32s31_hal::radio::domain::transaction::Fault) -> Self {
        Self(fault)
    }
    #[cfg(target_arch = "riscv32")]
    pub fn reset(self) -> ! {
        reset(self.0)
    }
}
