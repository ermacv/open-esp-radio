use oer_esp32s31_pac::{RxDcoControlField, TxDcPwdetFields, TxIqToneControlFields};

use super::{
    PhyRouteState, RxDcoControlPrepareError, RxDcoControlRestoreError, TxDcPwdetPrepareError,
    TxDcPwdetRestoreError, TxIqToneControlPrepareError, TxIqToneControlRestoreError,
};
use std::{cell::RefCell, vec::Vec};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestoreEvent {
    Capture,
    Prepare,
    Restore,
}

#[test]
fn txdc_restore_slot_rejects_interlopers_and_preserves_operation_order() {
    let mut slot = PhyRouteState::for_validation();
    let events = RefCell::new(Vec::new());
    slot.prepare_txdc_with(
        &mut (),
        |()| {
            events.borrow_mut().push(RestoreEvent::Capture);
            TxDcPwdetFields::default()
        },
        |()| events.borrow_mut().push(RestoreEvent::Prepare),
    )
    .unwrap();
    assert_eq!(
        events.borrow().as_slice(),
        [RestoreEvent::Capture, RestoreEvent::Prepare]
    );

    let rejected = slot.prepare_txdc_with(
        &mut (),
        |()| panic!("occupied restore slot must not capture registers"),
        |()| panic!("occupied restore slot must not prepare registers"),
    );
    assert_eq!(rejected, Err(TxDcPwdetPrepareError::RestorePending));
    assert_eq!(
        events.borrow().as_slice(),
        [RestoreEvent::Capture, RestoreEvent::Prepare]
    );
    let rejected = slot.prepare_txiq_with(|| panic!("TX-DC owner must exclude TX-IQ capture"));
    assert_eq!(rejected, Err(TxIqToneControlPrepareError::RestorePending));

    slot.restore_txdc_with(|_| {
        events.borrow_mut().push(RestoreEvent::Restore);
    })
    .unwrap();
    assert_eq!(
        events.borrow().as_slice(),
        [
            RestoreEvent::Capture,
            RestoreEvent::Prepare,
            RestoreEvent::Restore
        ]
    );

    let rejected =
        slot.restore_txdc_with(|_| panic!("empty restore slot must not touch registers"));
    assert_eq!(rejected, Err(TxDcPwdetRestoreError::RestoreNotPending));
}

#[test]
fn txiq_restore_slot_rejects_interlopers_and_consumes_authority_after_restore() {
    let mut slot = PhyRouteState::for_validation();
    let events = RefCell::new(Vec::new());
    slot.prepare_txiq_with(|| {
        events.borrow_mut().push(RestoreEvent::Capture);
        TxIqToneControlFields::default()
    })
    .unwrap();

    let rejected = slot.prepare_txiq_with(|| panic!("occupied slot must not sample the register"));
    assert_eq!(rejected, Err(TxIqToneControlPrepareError::RestorePending));
    assert_eq!(events.borrow().as_slice(), [RestoreEvent::Capture]);
    let rejected = slot.prepare_txdc_with(
        &mut (),
        |()| panic!("TX-IQ owner must exclude TX-DC capture"),
        |()| panic!("TX-IQ owner must exclude TX-DC preparation"),
    );
    assert_eq!(rejected, Err(TxDcPwdetPrepareError::RestorePending));

    slot.restore_txiq_with(|_| events.borrow_mut().push(RestoreEvent::Restore))
        .unwrap();
    assert_eq!(
        events.borrow().as_slice(),
        [RestoreEvent::Capture, RestoreEvent::Restore]
    );

    let rejected = slot.restore_txiq_with(|_| panic!("empty slot must not write the register"));
    assert_eq!(
        rejected,
        Err(TxIqToneControlRestoreError::RestoreNotPending)
    );
}

#[test]
fn rx_dco_restore_slot_is_a_bounded_lifo_and_excludes_other_calibrations() {
    let mut slot = PhyRouteState::for_validation();
    slot.prepare_rx_dco_with(|| RxDcoControlField::for_validation(1))
        .unwrap();
    slot.prepare_rx_dco_with(|| RxDcoControlField::for_validation(2))
        .unwrap();

    let rejected = slot.prepare_rx_dco_with(|| panic!("full stack must not capture"));
    assert_eq!(rejected, Err(RxDcoControlPrepareError::RestoreStackFull));
    let rejected = slot.prepare_txiq_with(|| panic!("RX-DCO must exclude TX-IQ capture"));
    assert_eq!(rejected, Err(TxIqToneControlPrepareError::RestorePending));

    let restored = RefCell::new(Vec::new());
    slot.restore_rx_dco_with(|field| restored.borrow_mut().push(field))
        .unwrap();
    assert!(slot.rx_dco_pending());
    slot.restore_rx_dco_with(|field| restored.borrow_mut().push(field))
        .unwrap();
    assert_eq!(
        restored.borrow().as_slice(),
        [
            RxDcoControlField::for_validation(2),
            RxDcoControlField::for_validation(1)
        ]
    );
    assert!(!slot.rx_dco_pending());

    let rejected = slot.restore_rx_dco_with(|_| panic!("empty stack must not write the register"));
    assert_eq!(rejected, Err(RxDcoControlRestoreError::RestoreNotPending));
}

#[test]
fn bluetooth_tx_power_restore_owns_the_slot_until_finished() {
    let mut slot = PhyRouteState::for_validation();
    assert!(slot.bluetooth_tx_power_control_values().is_err());
    slot.prepare_bluetooth_tx_power_control().unwrap();
    slot.capture_bluetooth_tx_power_control_low(3).unwrap();
    slot.capture_bluetooth_tx_power_control_high(7).unwrap();
    assert_eq!(slot.bluetooth_tx_power_control_values(), Ok((3, 7)));
    assert!(slot.prepare_bluetooth_tx_power_control().is_err());
    let rejected = slot.prepare_txiq_with(|| panic!("Bluetooth owner must exclude TX-IQ"));
    assert_eq!(rejected, Err(TxIqToneControlPrepareError::RestorePending));

    slot.finish_bluetooth_tx_power_control_restore().unwrap();
    assert!(!slot.bluetooth_tx_power_control_pending());
    assert!(slot.finish_bluetooth_tx_power_control_restore().is_err());
}
