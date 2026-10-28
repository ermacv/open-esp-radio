//! Fixed-size retained diagnostic codec shared by image compositions.
use super::{Fault, FaultReason, Operation, SarSetupFault, SarTransferFault};
use core::fmt;
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
            FaultReason::ModemClockIdentityExhausted => 19,
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
            FaultReason::SarAlarmWithoutBundle(_) => 18,
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
    pub fn report(&mut self, mut write: impl FnMut(&[u8]) -> bool) -> fmt::Result {
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
            19 => ("ModemClockIdentityExhausted", "none"),
            8 => ("OwnerUnavailable", "none"),
            9 => ("SarRequestOccupied", "maintain"),
            10 => ("SarProgramUnavailable", "maintain"),
            11 => ("SarInvalidDelay", "maintain"),
            12 => ("SarReplyOccupied", "maintain"),
            13 => ("SarResultUnavailable", "maintain"),
            14 => ("SarForeignSignal", "maintain"),
            15 => ("SarForeignReunion", "maintain"),
            16 => ("SarSetupStorage", "none"),
            18 => ("SarAlarmWithoutBundle", "none"),
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
        // Keep every output call static: `core::fmt::write` erases both the
        // writer and argument formatters, leaving unresolved boot-stack calls.
        let mut emit = |bytes: &[u8]| {
            if write(bytes) {
                Ok(())
            } else {
                Err(fmt::Error)
            }
        };
        let mut digits = [b'0'; u32::MAX.ilog10() as usize + 1];
        let mut registration = self.registration;
        for digit in digits.iter_mut().rev() {
            *digit = b'0'.wrapping_add((registration % 10) as u8);
            registration /= 10;
        }
        let mut decimal = digits.as_slice();
        while let [b'0', rest @ ..] = decimal {
            if rest.is_empty() {
                break;
            }
            decimal = rest;
        }
        emit(b"previous PHY transaction fault reason=")?;
        emit(reason.as_bytes())?;
        emit(b" operation=")?;
        emit(operation.as_bytes())?;
        emit(b" registration=")?;
        emit(decimal)?;
        emit(b"\n")?;
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
    fn exhausted_clock_identity_survives_refused_output() {
        let mut record: FaultRecord = bytemuck::Zeroable::zeroed();
        record.record(Fault {
            reason: FaultReason::ModemClockIdentityExhausted,
            registration: None,
        });
        assert!(record.report(|_| false).is_err());
        let mut bytes = std::vec::Vec::new();
        record
            .report(|chunk| {
                bytes.extend_from_slice(chunk);
                true
            })
            .unwrap();
        assert_eq!(bytes, b"previous PHY transaction fault reason=ModemClockIdentityExhausted operation=none registration=0\n");
        assert!(record.report(|_| panic!("reported once")).is_ok());
    }

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
    fn alarm_without_bundle_survives_retained_output_in_four_words() {
        let mut record: FaultRecord = bytemuck::Zeroable::zeroed();
        record.record(Fault {
            reason: FaultReason::SarAlarmWithoutBundle(super::super::SarAlarmWithoutBundle {
                observation: crate::platform::timg0_alarm::Observation {
                    pending: true,
                    asserted: false,
                },
                requests: 0,
                replies: 1,
            }),
            registration: None,
        });
        assert!(record.report(|_| false).is_err());
        let mut bytes = std::vec::Vec::new();
        record
            .report(|chunk| {
                bytes.extend_from_slice(chunk);
                true
            })
            .unwrap();
        let text = std::string::String::from_utf8(bytes).unwrap();
        assert!(text.contains("reason=SarAlarmWithoutBundle operation=none"));
        assert_eq!(core::mem::size_of::<FaultRecord>(), 16);
    }
    #[test]
    fn decimal_output_is_exact_and_each_refused_chunk_retains_the_record() {
        for registration in [0, 1, 9, 10, 99, 100, 999_999_999, 1_000_000_000, u32::MAX] {
            let mut original = FaultRecord {
                magic: MAGIC,
                reason: 1,
                registration,
                checksum: 0,
            };
            original.checksum = original.checksum();
            let expected = std::format!(
                "previous PHY transaction fault reason=Cancelled operation=register registration={registration}\n"
            );
            let mut record = original;
            let mut chunks = std::vec::Vec::new();
            record
                .report(|chunk| {
                    chunks.push(chunk.to_vec());
                    true
                })
                .unwrap();
            assert_eq!(chunks.concat(), expected.as_bytes());
            assert_eq!(record.magic, 0);
            for refused in 0..chunks.len() {
                let mut record = original;
                let mut attempted = 0;
                assert!(
                    record
                        .report(|_| {
                            let accepted = attempted != refused;
                            attempted += 1;
                            accepted
                        })
                        .is_err()
                );
                assert_eq!(attempted, refused + 1, "stop at the refused chunk");
                assert_eq!(record.magic, MAGIC, "retain until the newline succeeds");
                let mut replay = std::vec::Vec::new();
                record
                    .report(|chunk| {
                        replay.extend_from_slice(chunk);
                        true
                    })
                    .unwrap();
                assert_eq!(replay, expected.as_bytes());
                assert_eq!(record.magic, 0);
            }
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
