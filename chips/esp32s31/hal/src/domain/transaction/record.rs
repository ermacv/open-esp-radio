//! Fixed-size retained diagnostic codec shared by image compositions.
use super::{Fault, FaultReason, Operation, SarSetupFault, SarTransferFault};
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
            FaultReason::SarTransfer { operation, fault } => {
                let kind = match fault {
                    SarTransferFault::RequestOccupied { .. } => 9,
                    SarTransferFault::ProgramUnavailable => 10,
                    SarTransferFault::InvalidDelay { .. } => 11,
                    SarTransferFault::ReplyOccupied { .. } => 12,
                    SarTransferFault::ResultUnavailable => 13,
                    SarTransferFault::ForeignSignal => 14,
                    SarTransferFault::ForeignReunion { .. } => 15,
                };
                kind | (operation_code(operation) << 8)
            }
            FaultReason::SarSetup(SarSetupFault::Storage(_)) => 16,
            FaultReason::SarSetup(SarSetupFault::Binding(_)) => 17,
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
        let (reason, mut operation) = match self.reason & 0xff {
            1 => ("Cancelled", "register"),
            2 => ("Cancelled", "maintain"),
            3 => ("Cancelled", "wake"),
            4 => ("Cancelled", "close"),
            5 => ("Cancelled", "channel"),
            6 => ("Cancelled", "retune"),
            7 => ("IdentityClaimed", "none"),
            8 => ("OwnerUnavailable", "none"),
            9 => ("SarRequestOccupied", "maintain"),
            10 => ("SarProgramUnavailable", "maintain"),
            11 => ("SarInvalidDelay", "maintain"),
            12 => ("SarReplyOccupied", "maintain"),
            13 => ("SarResultUnavailable", "maintain"),
            14 => ("SarForeignSignal", "maintain"),
            15 => ("SarForeignReunion", "maintain"),
            16 => ("SarSetupStorage", "none"),
            17 => ("SarSetupBinding", "none"),
            _ => return Ok(()),
        };
        if (9..=15).contains(&(self.reason & 0xff)) {
            operation = match self.reason >> 8 {
                1 => "register",
                2 => "maintain",
                3 => "wake",
                4 => "close",
                5 => "channel",
                6 => "retune",
                _ => return Ok(()),
            };
        }
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
const fn operation_code(operation: Operation) -> u32 {
    match operation {
        Operation::Register => 1,
        Operation::Maintain => 2,
        Operation::Wake => 3,
        Operation::Close => 4,
        Operation::Channel => 5,
        Operation::Retune => 6,
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
    fn sar_transfer_keeps_the_admitted_operation_and_record_size() {
        for operation in [
            Operation::Register,
            Operation::Maintain,
            Operation::Wake,
            Operation::Close,
            Operation::Channel,
            Operation::Retune,
        ] {
            let mut record: FaultRecord = bytemuck::Zeroable::zeroed();
            record.record(Fault {
                reason: FaultReason::SarTransfer {
                    operation,
                    fault: SarTransferFault::RequestOccupied { queued: 1 },
                },
                registration: None,
            });
            let mut bytes = std::vec::Vec::new();
            record
                .report(|chunk| {
                    bytes.extend_from_slice(chunk);
                    true
                })
                .unwrap();
            let text = std::string::String::from_utf8(bytes).unwrap();
            assert!(text.contains("reason=SarRequestOccupied"));
            let expected = match operation {
                Operation::Register => "register",
                Operation::Maintain => "maintain",
                Operation::Wake => "wake",
                Operation::Close => "close",
                Operation::Channel => "channel",
                Operation::Retune => "retune",
            };
            assert!(text.contains(&std::format!("operation={expected}")));
            assert_eq!(core::mem::size_of::<FaultRecord>(), 16);
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
