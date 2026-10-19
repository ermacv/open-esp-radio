//! Test the compiled target evidence/control module, not a host reimplementation.
#![cfg(feature = "secure-gatt")]
use oer_hil_agent::bluetooth_gatt::secure::{state, store};
mod reset_gate_tests;
use oer_bluetooth_gatt_trouble::security::gatt::Observation;
use oer_hil_protocol::bluetooth::*;
use oer_hil_protocol::{base, bluetooth};

fn snapshot(state: &state::State) -> BluetoothSecureGattEvidence {
    state.snapshot().0
}

#[test]
fn application_failure_preserves_status_without_sensitive_host_payloads() {
    use oer_bluetooth_gatt_trouble::security::gatt::RunError;
    use trouble_host::{BleHostError, Error};
    let state = state::State::new();
    for (error, expected) in [
        (
            Error::Hci(bt_hci::param::Error::CMD_DISALLOWED),
            BluetoothGattApplicationFailure::HciStatus(0x0c),
        ),
        (
            Error::Disconnected,
            BluetoothGattApplicationFailure::Disconnected,
        ),
        (Error::Busy, BluetoothGattApplicationFailure::Busy),
        (
            Error::CannotConstructGattValue([0xa5; 16]),
            BluetoothGattApplicationFailure::HostOther,
        ),
    ] {
        state.application_failure(&RunError::<(), ()>::Host(BleHostError::BleHost(error)));
        assert_eq!(snapshot(&state).application_failure, Some(expected));
        assert!(!snapshot(&state).application_stopped);
        assert_eq!(snapshot(&state).cold_releases, 0);
    }
    state.restarted();
    assert!(snapshot(&state).application_failure.is_none());
}

#[test]
fn terminal_stop_clears_restart_intent_without_inventing_cold_release() {
    let state = state::State::new();
    let _ = state.restart_application(bluetooth::RestartGatt { epoch: 1 });
    assert!(snapshot(&state).restarting);
    let shutdown = BluetoothGattShutdown {
        cause: BluetoothGattStopCause::Application,
        reset: BluetoothGattResetOutcome::Completed,
    };
    state.shutdown(shutdown);
    state.stopped();
    let stopped = snapshot(&state);
    assert!(stopped.application_stopped);
    assert!(!stopped.restarting);
    assert_eq!((stopped.epoch, stopped.cold_releases), (1, 0));
    assert!(!stopped.old_hci_closed);
    assert_eq!(stopped.shutdown, Some(shutdown));
    state.cold(true);
    let cold = snapshot(&state);
    assert_eq!((cold.epoch, cold.cold_releases), (1, 1));
    assert!(cold.application_stopped && cold.old_hci_closed);
    assert!(matches!(
        state.restart_application(bluetooth::RestartGatt { epoch: 1 }),
        Err(base::RejectReason::InvalidState)
    ));
}

#[test]
fn injected_load_failure_is_single_use_and_preserves_the_real_ram_record() {
    use oer_bluetooth_gatt_trouble::security::bonds::{BondStore, RamBondStore, StoreError};
    use trouble_host::{Address, BondInformation, Identity, LongTermKey, prelude::SecurityLevel};
    fn ready<F: Future>(future: F) -> F::Output {
        match core::pin::pin!(future).poll(&mut core::task::Context::from_waker(
            core::task::Waker::noop(),
        )) {
            core::task::Poll::Ready(value) => value,
            core::task::Poll::Pending => panic!("RAM operation blocked"),
        }
    }
    let state = state::State::new();
    let command = bluetooth::FailNextGattBondLoad { epoch: 1 };
    assert!(matches!(state.fail_next_bond_load(command.clone()), Err(_)));
    state.observe(Observation::Connected);
    assert!(matches!(state.fail_next_bond_load(command.clone()), Err(_)));
    let mut ram = RamBondStore::<1>::new();
    let bond = BondInformation::new(
        Identity::from(Address::random([1, 2, 3, 4, 5, 0xc6])),
        LongTermKey::new(123),
        SecurityLevel::EncryptedAuthenticated,
        true,
    );
    let mut store = store::Store {
        ram: &mut ram,
        state: &state,
    };
    ready(store.insert(bond.clone())).unwrap();
    state.observe(Observation::BondStored);
    assert!(matches!(
        state.fail_next_bond_load(bluetooth::FailNextGattBondLoad { epoch: 0 }),
        Err(_)
    ));
    assert!(matches!(state.fail_next_bond_load(command.clone()), Ok(_)));
    assert_eq!(snapshot(&state).bond_load_failures, 0);
    assert!(snapshot(&state).shutdown.is_none());
    assert!(matches!(state.fail_next_bond_load(command.clone()), Err(_)));
    assert!(matches!(
        state.restart_application(bluetooth::RestartGatt { epoch: 1 }),
        Err(_)
    ));
    // Merely creating an unpolled read must not consume the injection.
    drop(store.load(0));
    assert!(snapshot(&state).bond_load_fault_armed);
    assert!(matches!(
        ready(store.load(0)),
        Err(StoreError::Backend(store::InjectedBondLoadFailure))
    ));
    assert_eq!(snapshot(&state).bond_load_failures, 1);
    assert!(!snapshot(&state).application_stopped);
    assert_eq!(snapshot(&state).cold_releases, 0);
    assert!(ready(store.load(0)).unwrap().as_ref() == Some(&bond));
    assert!(matches!(state.fail_next_bond_load(command), Err(_)));
}
#[test]
fn explicit_decision_is_single_use_and_not_bond_evidence() {
    let state = state::State::new();
    let lease = state.comparison.begin(123).unwrap();
    let challenge = snapshot(&state).comparison.unwrap();
    let decision = BluetoothNumericDecision {
        challenge,
        accept: true,
    };
    assert_eq!(
        state.confirm(bluetooth::ConfirmGatt(decision)),
        Ok(bluetooth::GattDecisionRecorded(decision))
    );
    assert!(snapshot(&state).comparison.is_none());
    assert_eq!(snapshot(&state).bonds_stored, 0);
    assert_eq!(snapshot(&state).accepted, 0);
    assert!(matches!(
        state.confirm(bluetooth::ConfirmGatt(decision)),
        Err(_)
    ));
    drop(lease);
    let _next = state.comparison.begin(123).unwrap();
    assert!(matches!(
        state.confirm(bluetooth::ConfirmGatt(decision)),
        Err(_)
    ));
    state.stopped();
    let decision = BluetoothNumericDecision {
        challenge: snapshot(&state).comparison.unwrap(),
        accept: true,
    };
    assert!(matches!(
        state.confirm(bluetooth::ConfirmGatt(decision)),
        Err(_)
    ));
}
#[test]
fn observations_preserve_epoch_counters_without_inventing_peer_delivery() {
    let state = state::State::new();
    for event in [
        Observation::Ready([1; 6]),
        Observation::Advertising,
        Observation::Connected,
        Observation::BondStored,
        Observation::Written(42),
        Observation::NotificationQueued(42),
        Observation::Disconnected(0x13),
        Observation::Advertising,
        Observation::Connected,
        Observation::BondResumed,
        Observation::Read(42),
    ] {
        state.observe(event);
    }
    let e = snapshot(&state);
    assert_eq!((e.traffic.connections, e.traffic.disconnections), (2, 1));
    assert_eq!(
        (e.bonds_stored, e.bonds_resumed, e.notifications_queued),
        (1, 1, 1)
    );
    assert_eq!(
        (e.traffic.value, e.traffic.reads, e.traffic.writes),
        (42, 1, 1)
    );
    assert!(e.comparison.is_none());
    assert!(!e.application_stopped);
}

#[test]
fn cold_restart_is_epoch_bound_and_does_not_reset_bond_or_comparison_history() {
    let state = state::State::new();
    state.observe(Observation::BondStored);
    state.observe(Observation::Written(42));
    let prompt = state.comparison.begin(123).unwrap();
    let challenge = snapshot(&state).comparison.unwrap();
    let decision = BluetoothNumericDecision {
        challenge,
        accept: true,
    };
    assert!(matches!(
        state.restart_application(bluetooth::RestartGatt { epoch: 0 }),
        Err(_)
    ));
    assert!(
        matches!(state.restart_application(bluetooth::RestartGatt { epoch: 1 }), Ok(bluetooth::SecureGattState(e)) if e.restarting)
    );
    assert!(matches!(
        state.restart_application(bluetooth::RestartGatt { epoch: 1 }),
        Err(_)
    ));
    assert!(matches!(
        state.confirm(bluetooth::ConfirmGatt(decision)),
        Err(_)
    ));
    drop(prompt);
    state.cold(true);
    state.restarted();
    let e = snapshot(&state);
    assert_eq!(
        (e.epoch, e.cold_releases, e.bonds_stored, e.traffic.value),
        (2, 1, 1, 0)
    );
    assert!(e.old_hci_closed);
    assert!(!e.restarting);
    assert!(matches!(
        state.restart_application(bluetooth::RestartGatt { epoch: 1 }),
        Err(_)
    ));
    let _new = state.comparison.begin(123).unwrap();
    assert!(matches!(
        state.confirm(bluetooth::ConfirmGatt(decision)),
        Err(_)
    ));
    state.stopped();
    assert!(matches!(
        state.restart_application(bluetooth::RestartGatt { epoch: 2 }),
        Err(_)
    ));
    assert!(snapshot(&state).application_stopped);
}
