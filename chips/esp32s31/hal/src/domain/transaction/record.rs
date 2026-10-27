//! Fixed-size retained diagnostic codec shared by image compositions.
use super::{Fault, FaultReason, Operation};
use core::fmt::{self, Write};
const MAGIC: u32 = 0x5054_5802;

/// Sixteen retained bytes: version, operation/reason, epoch and checksum.
#[derive(Clone, Copy, bytemuck::AnyBitPattern)]
#[repr(C)]
pub struct FaultRecord {
    magic: u32,
    reason: u32,
    registration: u32,
    checksum: u32,
}
impl FaultRecord {
    pub fn record(&mut self, fault: Fault) {
        self.magic = 0;
        self.reason = match fault.reason {
            FaultReason::Cancelled(Operation::Register) => 1,
            FaultReason::Cancelled(Operation::Maintain) => 2,
            FaultReason::Cancelled(Operation::Wake) => 3,
            FaultReason::Cancelled(Operation::Close) => 4,
            FaultReason::Cancelled(Operation::Channel) => 5,
            FaultReason::Cancelled(Operation::Retune) => 6,
            FaultReason::IdentityClaimed => 7,
            FaultReason::OwnerUnavailable => 8,
        };
        self.registration = fault.registration.map_or(0, |epoch| epoch.get());
        self.checksum = self.checksum();
        self.magic = MAGIC;
    }
    fn checksum(&self) -> u32 {
        [self.reason, self.registration]
            .iter()
            .fold(MAGIC, |sum, word| (sum ^ word).wrapping_mul(0x0100_0193))
    }
    /// Report a valid record; failed output retains it for the next boot.
    pub fn report(&mut self, write: impl FnMut(&[u8]) -> bool) -> fmt::Result {
        if self.magic != MAGIC || self.checksum != self.checksum() {
            return Ok(());
        }
        let (reason, operation) = match self.reason {
            1 => ("Cancelled", "register"),
            2 => ("Cancelled", "maintain"),
            3 => ("Cancelled", "wake"),
            4 => ("Cancelled", "close"),
            5 => ("Cancelled", "channel"),
            6 => ("Cancelled", "retune"),
            7 => ("IdentityClaimed", "none"),
            8 => ("OwnerUnavailable", "none"),
            _ => return Ok(()),
        };
        struct Output<F>(F);
        impl<F: FnMut(&[u8]) -> bool> Write for Output<F> {
            fn write_str(&mut self, text: &str) -> fmt::Result {
                if (self.0)(text.as_bytes()) {
                    Ok(())
                } else {
                    Err(fmt::Error)
                }
            }
        }
        writeln!(
            Output(write),
            "previous PHY transaction fault reason={reason} operation={operation} registration={}",
            self.registration
        )?;
        self.magic = 0;
        Ok(())
    }
}
const _: () = assert!(core::mem::size_of::<FaultRecord>() == 16);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn record_is_four_words() {
        assert_eq!(core::mem::size_of::<FaultRecord>(), 16);
    }
    #[test]
    fn failed_output_retains_operation_and_epoch_until_reported() {
        for (operation, name) in [
            (Operation::Register, "register"),
            (Operation::Maintain, "maintain"),
            (Operation::Wake, "wake"),
            (Operation::Close, "close"),
            (Operation::Channel, "channel"),
            (Operation::Retune, "retune"),
        ] {
            let mut record: FaultRecord = bytemuck::Zeroable::zeroed();
            let mut registration = crate::phy::registration::PhyRegistration::new();
            let epoch = registration.begin();
            record.record(Fault {
                reason: FaultReason::Cancelled(operation),
                registration: Some(epoch),
            });
            assert!(record.report(|_| false).is_err());
            let mut bytes = std::vec::Vec::new();
            assert!(
                record
                    .report(|chunk| {
                        bytes.extend_from_slice(chunk);
                        true
                    })
                    .is_ok()
            );
            let text = std::string::String::from_utf8(bytes).unwrap();
            assert!(text.contains("reason=Cancelled"));
            assert!(text.contains(&std::format!("operation={name}")));
            assert!(text.contains(&std::format!("registration={}", epoch.get())));
            assert_eq!(record.magic, 0);
            assert!(record.report(|_| panic!("reported once")).is_ok());
        }
    }
    #[test]
    fn corrupted_record_is_not_reported() {
        let mut record: FaultRecord = bytemuck::Zeroable::zeroed();
        record.record(Fault {
            reason: FaultReason::Cancelled(Operation::Retune),
            registration: None,
        });
        record.registration ^= 1;
        assert!(record.report(|_| panic!("corrupt record")).is_ok());
    }
}
