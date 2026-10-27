use super::*;
fn quiet() -> BluetoothInterruptPolicyObservation {
    BluetoothInterruptPolicyObservation {
        primary: BluetoothInterruptObservation {
            enable_0: 0,
            enable_1: 0,
            status_0: 0,
            status_1: 0,
            nrt_0: 0,
            nrt_1: 0,
        },
        task_enable: 0,
    }
}
#[test]
fn every_foreign_enable_fault_keeps_all_banks() {
    for bank in 0..3 {
        for lane in 0..u32::BITS {
            let mut observed = quiet();
            let bit = 1u32 << lane;
            let allowed = match bank {
                0 => {
                    observed.primary.enable_0 = bit;
                    PRODUCTION.baseline_0.0 | PRODUCTION.dynamic_0.0
                }
                1 => {
                    observed.primary.enable_1 = bit;
                    PRODUCTION.baseline_1.0 | PRODUCTION.dynamic_1.0
                }
                _ => {
                    observed.task_enable = bit;
                    PRODUCTION.task.0
                }
            };
            observed.primary.nrt_0 = 0x12345678;
            observed.primary.status_1 = 0x87654321;
            if bit & allowed == 0 {
                assert_eq!(
                    observed.admit(),
                    Err(BluetoothInterruptPolicyFault::ForeignEnable(observed))
                );
            } else {
                assert_eq!(observed.admit(), Ok(()));
            }
        }
    }
}
#[test]
fn release_requires_every_enable_and_pending_view_to_be_zero() {
    assert_eq!(quiet().confirm_released(), Ok(()));
    for bank in 0..7 {
        let mut observed = quiet();
        let words = [
            &mut observed.primary.enable_0,
            &mut observed.primary.enable_1,
            &mut observed.primary.status_0,
            &mut observed.primary.status_1,
            &mut observed.primary.nrt_0,
            &mut observed.primary.nrt_1,
            &mut observed.task_enable,
        ];
        *words.into_iter().nth(bank).unwrap() = 0x80000001;
        assert_eq!(
            observed.confirm_released(),
            Err(BluetoothInterruptPolicyFault::ReleaseNotQuiet(observed))
        );
    }
}
