//! The PHY source payload in the common diagnostic envelope. No storage owner.
use super::{Fault, FaultReason, Operation, SarSetupFault, SarTransferFault};
use core::fmt;
use oer_diagnostics::fault::{Class, Domain, Error, Record};
/// Encode every source observation without truncation.
pub fn encode_fault(fault: Fault) -> Result<Record, Error> {
    let reason = match fault.reason {
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
    let epoch = fault.registration.map_or(0, |epoch| epoch.get());
    let mut words = [epoch, 0, 0, 0, 0];
    let class = match fault.reason {
        FaultReason::Cancelled(_) => Class::Cancellation,
        FaultReason::ModemClockIdentityExhausted => Class::Exhaustion,
        FaultReason::SarSetup(SarSetupFault::Storage(storage)) => {
            let [_, a, _, _, _] = &mut words;
            *a = match storage {
                super::SarSetupStorage::Cells => 1,
                super::SarSetupStorage::State => 2,
                super::SarSetupStorage::Handler => 3,
            };
            Class::ConfigurationBoot
        }
        FaultReason::SarAlarmWithoutBundle(sample) => {
            words = [
                epoch,
                u32::from(sample.observation.pending),
                u32::from(sample.observation.asserted),
                u32::try_from(sample.requests).map_err(|_| Error::Overflow {
                    length: sample.requests,
                })?,
                u32::try_from(sample.replies).map_err(|_| Error::Overflow {
                    length: sample.replies,
                })?,
            ];
            Class::UnexpectedHardware
        }
        FaultReason::SarTransfer { fault, .. } => {
            let [_, a, b, _, _] = &mut words;
            match fault {
                SarTransferFault::RequestOccupied { queued }
                | SarTransferFault::ReplyOccupied { queued } => {
                    *a = u32::try_from(queued).map_err(|_| Error::Overflow { length: queued })?;
                }
                SarTransferFault::InvalidDelay { requested } => {
                    let ticks = requested.as_micros();
                    *a = ticks as u32;
                    *b = (ticks >> 32) as u32;
                }
                SarTransferFault::ForeignReunion {
                    expected_owner,
                    received_owner,
                } => {
                    *a = u32::try_from(expected_owner).map_err(|_| Error::Overflow {
                        length: expected_owner,
                    })?;
                    *b = u32::try_from(received_owner).map_err(|_| Error::Overflow {
                        length: received_owner,
                    })?;
                }
                _ => {}
            }
            Class::SoftwareInvariant
        }
        _ => Class::SoftwareInvariant,
    };
    Record::encode(
        class,
        Domain::Phy,
        reason,
        oer_diagnostics::fault::UNKNOWN_HART,
        0,
        &words,
    )
}
/// Output acknowledgement is the only operation that clears the record.
pub fn report_fault(record: &mut Record, mut write: impl FnMut(&[u8]) -> bool) -> fmt::Result {
    if record.peek().is_err() {
        return Ok(());
    }
    record
        .report(|view| -> fmt::Result {
            let words: [u32; 5] = view.decode(Domain::Phy).map_err(|_| fmt::Error)?;
            let [registration, ..] = words;
            let (reason, mut operation) = match view.reason & 0xff {
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
                _ => return Err(fmt::Error),
            };
            if (9..=15).contains(&(view.reason & 0xff)) {
                operation = match view.reason >> 8 {
                    1 => "register",
                    2 => "maintain",
                    3 => "wake",
                    4 => "close",
                    5 => "channel",
                    6 => "retune",
                    _ => return Err(fmt::Error),
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
            let mut registration = registration;
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
            emit(b" observations=")?;
            for word in words {
                for byte in word.to_be_bytes() {
                    let hex = b"0123456789abcdef";
                    let hi = hex.get(usize::from(byte >> 4)).copied().ok_or(fmt::Error)?;
                    let lo = hex.get(usize::from(byte & 15)).copied().ok_or(fmt::Error)?;
                    emit(&[hi, lo])?;
                }
            }
            emit(b"\n")?;
            Ok(())
        })
        .map_err(|_| fmt::Error)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sar_snapshot_and_epoch_survive_refused_output() {
        let fault = Fault {
            reason: FaultReason::SarAlarmWithoutBundle(super::super::SarAlarmWithoutBundle {
                observation: crate::peripherals::timg0_alarm::Observation {
                    pending: true,
                    asserted: false,
                },
                requests: 0,
                replies: 1,
            }),
            registration: None,
        };
        let mut record = encode_fault(fault).unwrap();
        assert_eq!(
            record
                .peek()
                .unwrap()
                .decode::<[u32; 5]>(Domain::Phy)
                .unwrap(),
            [0, 1, 0, 0, 1]
        );
        assert!(report_fault(&mut record, |_| false).is_err());
        assert!(record.peek().is_ok());
        report_fault(&mut record, |_| true).unwrap();
        assert!(record.peek().is_err());
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;

    #[test]
    fn cancellation_keeps_each_operation_and_registration_after_refused_output() {
        for (operation, name) in [
            (Operation::Register, "register"),
            (Operation::Maintain, "maintain"),
            (Operation::Wake, "wake"),
            (Operation::Close, "close"),
            (Operation::Channel, "channel"),
            (Operation::Retune, "retune"),
        ] {
            let mut registration = crate::phy::registration::PhyRegistration::new();
            let epoch = registration.begin();
            let mut record = encode_fault(Fault {
                reason: FaultReason::Cancelled(operation),
                registration: Some(epoch),
            })
            .unwrap();
            assert_eq!(record.peek().unwrap().class, Class::Cancellation);
            assert_eq!(
                record
                    .peek()
                    .unwrap()
                    .decode::<[u32; 5]>(Domain::Phy)
                    .unwrap(),
                [epoch.get(), 0, 0, 0, 0]
            );
            assert!(report_fault(&mut record, |_| false).is_err());
            let mut bytes = std::vec::Vec::new();
            report_fault(&mut record, |part| {
                bytes.extend_from_slice(part);
                true
            })
            .unwrap();
            let text = std::string::String::from_utf8(bytes).unwrap();
            assert!(text.contains(&std::format!(
                "reason=Cancelled operation={name} registration={}",
                epoch.get()
            )));
            assert!(record.peek().is_err());
        }
    }

    #[test]
    fn sar_transfer_retains_queue_lengths_delay_and_numeric_reunion_observations() {
        let cases = [
            (
                SarTransferFault::RequestOccupied {
                    queued: u32::MAX as usize,
                },
                [u32::MAX, 0],
            ),
            (SarTransferFault::ReplyOccupied { queued: 1 }, [1, 0]),
            (
                SarTransferFault::InvalidDelay {
                    requested: oer_time::Duration::from_micros(u64::MAX),
                },
                [u32::MAX, u32::MAX],
            ),
            (
                SarTransferFault::ForeignReunion {
                    expected_owner: 0x12345678,
                    received_owner: 0xfedcba98,
                },
                [0x12345678, 0xfedcba98],
            ),
        ];
        for (fault, expected) in cases {
            let record = encode_fault(Fault {
                reason: FaultReason::SarTransfer {
                    operation: Operation::Retune,
                    fault,
                },
                registration: None,
            })
            .unwrap();
            let [epoch, a, b, c, d] = record
                .peek()
                .unwrap()
                .decode::<[u32; 5]>(Domain::Phy)
                .unwrap();
            assert_eq!([a, b], expected);
            assert_eq!([epoch, c, d], [0; 3]);
        }
    }

    #[test]
    fn an_unrepresentable_host_observation_is_refused_without_truncation() {
        if let Some(queued) = (u32::MAX as usize).checked_add(1) {
            assert!(
                matches!(encode_fault(Fault { reason: FaultReason::SarTransfer {
                operation: Operation::Maintain, fault: SarTransferFault::RequestOccupied { queued },
            }, registration: None }), Err(Error::Overflow { length }) if length == queued)
            );
        }
    }
}
