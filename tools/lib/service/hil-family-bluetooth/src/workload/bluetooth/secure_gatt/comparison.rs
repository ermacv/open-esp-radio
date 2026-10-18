//! Automated HIL comparison of independent DUT and Linux challenges.
//! This is a test operator, not a production user-consent policy.
use crate::Result;
use oer_hil_protocol::{bluetooth::BluetoothNumericChallenge, bluetooth::BluetoothNumericDecision};

pub(super) fn decision(
    boot: u64,
    dut: BluetoothNumericChallenge,
    linux_number: u32,
    accept: bool,
) -> Result<BluetoothNumericDecision> {
    if boot == 0 || dut.id == 0 {
        return Err("Numeric Comparison requires a live boot and request identity".into());
    }
    if dut.number > 999_999 || linux_number != dut.number {
        return Err("peer and DUT Numeric Comparison mismatch or invalid number".into());
    }
    Ok(BluetoothNumericDecision {
        challenge: dut,
        accept,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_matching_numbers_preserve_the_exact_request_and_test_decision() {
        for number in [0, 123, 999_999] {
            let dut = BluetoothNumericChallenge { id: 7, number };
            for accept in [false, true] {
                let result = decision(42, dut, number, accept).unwrap();
                assert_eq!(result.challenge, dut);
                assert_eq!(result.accept, accept);
            }
        }
    }

    #[test]
    fn no_decision_for_unbound_mismatched_or_invalid_challenges() {
        let dut = BluetoothNumericChallenge { id: 7, number: 123 };
        for accept in [false, true] {
            assert!(decision(0, dut, 123, accept).is_err());
            assert!(decision(42, BluetoothNumericChallenge { id: 0, ..dut }, 123, accept).is_err());
            assert!(decision(42, dut, 124, accept).is_err());
            assert!(
                decision(
                    42,
                    BluetoothNumericChallenge {
                        number: 1_000_000,
                        ..dut
                    },
                    1_000_000,
                    accept
                )
                .is_err()
            );
        }
    }
}
