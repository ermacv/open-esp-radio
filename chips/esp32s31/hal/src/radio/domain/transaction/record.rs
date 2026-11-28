//! The PHY source payload in the common diagnostic envelope. No storage owner.
use super::{Fault, FaultReason, Operation};
use core::fmt;
use oer_portable_diagnostics::fault::{Class, Domain, Error, Record};
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
    };
    let epoch = fault.registration.map_or(0, |epoch| epoch.get());
    let words = [epoch, 0, 0, 0, 0];
    let class = match fault.reason {
        FaultReason::Cancelled(_) => Class::Cancellation,
        FaultReason::ModemClockIdentityExhausted => Class::Exhaustion,
        FaultReason::IdentityClaimed | FaultReason::OwnerUnavailable => Class::SoftwareInvariant,
    };
    Record::encode(
        class,
        Domain::Phy,
        reason,
        oer_portable_diagnostics::fault::UNKNOWN_HART,
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
            let (reason, operation) = match view.reason & 0xff {
                1 => ("Cancelled", "register"),
                2 => ("Cancelled", "maintain"),
                3 => ("Cancelled", "wake"),
                4 => ("Cancelled", "close"),
                5 => ("Cancelled", "channel"),
                6 => ("Cancelled", "retune"),
                7 => ("IdentityClaimed", "none"),
                19 => ("ModemClockIdentityExhausted", "none"),
                8 => ("OwnerUnavailable", "none"),
                _ => return Err(fmt::Error),
            };
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
            let mut registration = crate::radio::phy::registration::PhyRegistration::new();
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
}
