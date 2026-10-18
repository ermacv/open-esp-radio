use super::*;
use crate::{
    bluetooth::BluetoothNumericChallenge, bluetooth::BluetoothNumericDecision,
    bluetooth::BluetoothSecureGattEvidence,
};

#[test]
fn reset_read_failure_request_preserves_epoch_and_boot_binding() {
    let expected = Envelope::new(
        77,
        9,
        0,
        8,
        crate::bluetooth::FailGattResetRead { epoch: u32::MAX },
    );
    let mut encoder = FrameEncoder::new();
    let bytes = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, bytes).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn reset_reader_control_is_boot_bound_and_preserves_epoch_and_operation() {
    for release in [false, true] {
        let expected = Envelope::new(
            77,
            9,
            0,
            8,
            crate::bluetooth::GattResetReadGate {
                epoch: u32::MAX,
                release,
            },
        );
        let mut encoder = FrameEncoder::new();
        let bytes = encoder.encode(&expected).unwrap();
        let mut decoder = FrameDecoder::new();
        let observed = receive(&mut decoder, bytes).map(Result::unwrap);
        assert_eq!(observed, Some(expected));
    }
}

#[test]
fn secure_restart_is_boot_bound_and_preserves_requested_epoch() {
    let expected = Envelope::new(
        77,
        9,
        0,
        8,
        crate::bluetooth::RestartGatt { epoch: u32::MAX },
    );
    let mut encoder = FrameEncoder::new();
    let bytes = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, bytes).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn secure_decision_is_boot_bound_and_round_trips_without_key_material() {
    let decision = BluetoothNumericDecision {
        challenge: BluetoothNumericChallenge {
            id: u64::MAX,
            number: 999_999,
        },
        accept: true,
    };
    let expected = Envelope::new(77, 9, 0, 8, crate::bluetooth::ConfirmGatt(decision));
    let mut encoder = FrameEncoder::new();
    let bytes = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, bytes).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}
#[test]
fn secure_evidence_retains_independent_ui_bond_and_delivery_counts() {
    let expected = Envelope::new(
        u64::MAX,
        u32::MAX,
        0,
        u32::MAX,
        crate::bluetooth::SecureGattState(BluetoothSecureGattEvidence {
            advertising_start_rejection: Some(
                crate::bluetooth::BluetoothAdvertisingStartRejection::TimingWindow,
            ),
            application_failure: Some(
                crate::bluetooth::BluetoothGattApplicationFailure::HciStatus(u8::MAX),
            ),
            reset_read_gate: crate::bluetooth::BluetoothGattResetReadGate::ReaderHeld,
            bond_load_fault_armed: true,
            bond_load_failures: u32::MAX,
            shutdown: Some(crate::bluetooth::BluetoothGattShutdown {
                cause: crate::bluetooth::BluetoothGattStopCause::InjectedBondLoadFailure,
                reset: crate::bluetooth::BluetoothGattResetOutcome::Completed,
            }),
            epoch: u32::MAX,
            restarting: true,
            cold_releases: u32::MAX,
            old_hci_closed: true,
            comparison: Some(BluetoothNumericChallenge {
                id: u64::MAX,
                number: 999_999,
            }),
            comparisons: u32::MAX,
            accepted: u32::MAX,
            declined: u32::MAX,
            bonds_stored: u32::MAX,
            bonds_resumed: u32::MAX,
            pairing_failures: u32::MAX,
            rejected: u32::MAX,
            notifications_queued: u32::MAX,
            application_stopped: true,
            traffic: crate::bluetooth::BluetoothGattEvidence {
                address: Some([255; 6]),
                connections: u32::MAX,
                disconnections: u32::MAX,
                reads: u32::MAX,
                writes: u32::MAX,
                advertising_starts: u32::MAX,
                value: 255,
                last_disconnect_reason: Some(255),
                cpu0_stack: Some(crate::system::StackWatermark {
                    capacity_bytes: u32::MAX,
                    free_bytes: u32::MAX,
                    used_bytes: u32::MAX,
                    minimum_free_bytes: u32::MAX,
                }),
                ..Default::default()
            },
        }),
    );
    let mut encoder = FrameEncoder::new();
    let bytes = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, bytes).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}

#[test]
fn bond_load_fault_is_boot_bound_and_round_trips() {
    let expected = Envelope::new(
        77,
        9,
        0,
        8,
        crate::bluetooth::FailNextGattBondLoad { epoch: u32::MAX },
    );
    let mut encoder = FrameEncoder::new();
    let bytes = encoder.encode(&expected).unwrap();
    let mut decoder = FrameDecoder::new();
    let observed = receive(&mut decoder, bytes).map(Result::unwrap);
    assert_eq!(observed, Some(expected));
}
